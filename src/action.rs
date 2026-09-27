//! One explicit SIGTERM operation for the Windows TUI's resolved primary WSL.
use crate::command::{output_with_timeout, CommandSpec};
use crate::model::{EnvironmentKind, ProcessKey, ResourceKind, ResourceUsage};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub distro: String,
    pub boot: String,
    pub pid_namespace: String,
    pub mount_namespace: String,
    pub uid: String,
}

impl Scope {
    pub fn parse(distro: &str, boot: &str, text: &str) -> Option<Self> {
        let fields: Vec<_> = text.split_whitespace().collect();
        if distro.is_empty()
            || boot.is_empty()
            || fields.len() != 3
            || !fields[0].starts_with("pid:[")
            || !fields[1].starts_with("mnt:[")
            || fields[2].parse::<u32>().is_err()
        {
            return None;
        }
        Some(Self {
            distro: distro.into(),
            boot: boot.into(),
            pid_namespace: fields[0].into(),
            mount_namespace: fields[1].into(),
            uid: fields[2].into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub scope: Scope,
    pub key: ProcessKey,
    pub name: String,
}

impl Request {
    pub fn new(scope: Option<&Scope>, row: &ResourceUsage) -> Result<Self, &'static str> {
        let scope = scope.ok_or("Termination unavailable: primary WSL identity is not ready")?;
        if row.environment != EnvironmentKind::Wsl
            || row.kind != ResourceKind::Process
            || row.source.is_some()
        {
            return Err(
                "Unsupported target: only ordinary primary WSL processes can be terminated",
            );
        }
        let pid = row
            .pid
            .filter(|pid| *pid > 1)
            .ok_or("Unsupported target: missing PID or init process")?;
        let start_id = row
            .start_id
            .filter(|start| *start > 0)
            .ok_or("Unverifiable target: process start identity is unavailable")?;
        Ok(Self {
            scope: scope.clone(),
            key: ProcessKey {
                environment: row.environment,
                source: None,
                pid,
                start_id,
            },
            name: row
                .name
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
        })
    }
}

pub fn terminate(request: Request) -> String {
    let script = include_str!("terminate.py");
    // No shell, stdin protocol or user-provided code. A whitespace-free loader
    // preserves the fixed script through wsl.exe's Windows argument forwarding.
    let hex: String = script.bytes().map(|b| format!("{b:02x}")).collect();
    let loader = format!("exec(bytes.fromhex('{hex}'))");
    let pid = request.key.pid.to_string();
    let start = request.key.start_id.to_string();
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_add(10)
        .to_string();
    let args = [
        "--distribution",
        &request.scope.distro,
        "--exec",
        "python3",
        "-I",
        "-c",
        &loader,
        &pid,
        &start,
        &request.scope.boot,
        &request.scope.pid_namespace,
        &request.scope.mount_namespace,
        &request.scope.uid,
        &deadline,
    ];
    match output_with_timeout(CommandSpec::new("wsl.exe", &args), Duration::from_secs(12)) {
        Ok(output) => match String::from_utf8_lossy(&output.stdout).trim() {
            "WSLTOP_ACTION:accepted" if output.status.success() =>
                "SIGTERM accepted; the process may still be running. Check subsequent observations.".into(),
            "WSLTOP_ACTION:stale" => "Target disappeared or its identity changed; no signal sent.".into(),
            "WSLTOP_ACTION:denied" => "Permission denied; no privilege escalation attempted.".into(),
            "WSLTOP_ACTION:unsupported" => "Termination requires Python 3.9+ with pidfd support in the target WSL.".into(),
            "WSLTOP_ACTION:expired" => "Termination deadline expired; no signal sent.".into(),
            _ => "Dispatch failed or result unavailable. Requires Python 3.9+ and pidfd in WSL; check the target before retrying.".into(),
        },
        Err(_) => "Dispatch failed or timed out; outcome unknown. Check the target before retrying.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn scope() -> Scope {
        Scope::parse("Ubuntu", "boot", "pid:[7] mnt:[8] 1000").unwrap()
    }

    #[test]
    fn rejects_unsupported_or_unverifiable_rows() {
        let mut row = crate::query::tests::row("worker", 1.0, 1);
        row.environment = EnvironmentKind::Wsl;
        row.kind = ResourceKind::Process;
        row.pid = Some(99);
        row.start_id = Some(123);
        row.source = None;
        assert!(Request::new(Some(&scope()), &row).is_ok());
        assert!(Request::new(None, &row).is_err());
        for environment in [
            EnvironmentKind::Windows,
            EnvironmentKind::Docker,
            EnvironmentKind::WslContainer,
        ] {
            let mut other = row.clone();
            other.environment = environment;
            assert!(Request::new(Some(&scope()), &other).is_err());
        }
        for kind in [
            ResourceKind::Application,
            ResourceKind::Host,
            ResourceKind::Infra,
            ResourceKind::Container,
        ] {
            let mut other = row.clone();
            other.kind = kind;
            assert!(Request::new(Some(&scope()), &other).is_err());
        }
        let mut other = row.clone();
        other.source = Some("other-distro".into());
        assert!(Request::new(Some(&scope()), &other).is_err());
        for start in [None, Some(0)] {
            row.start_id = start;
            assert!(Request::new(Some(&scope()), &row).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn embedded_helper_validation_and_live_pidfd_regressions() {
        let output = std::process::Command::new("python3")
            .args([
                "-I",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/terminate_backend.py"),
            ])
            .output()
            .expect("Python 3 is needed to test the embedded action helper");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an explicitly selected live WSL distribution; uses a disposable 30-second child"]
    fn live_windows_to_wsl_termination() {
        let distro =
            std::env::var("WSLTOP_ACTION_TEST_DISTRO").expect("set test distro explicitly");
        let setup = "import subprocess; p=subprocess.Popen(['python3','-I','-c','import time; time.sleep(30)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True); print(p.pid)";
        let hex: String = setup.bytes().map(|b| format!("{b:02x}")).collect();
        let loader = format!("exec(bytes.fromhex('{hex}'))");
        let output = output_with_timeout(
            CommandSpec::new(
                "wsl.exe",
                &[
                    "--distribution",
                    &distro,
                    "--exec",
                    "python3",
                    "-I",
                    "-c",
                    &loader,
                ],
            ),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pid: u32 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        let snapshot = crate::multiwsl::snapshot(&distro, None).unwrap();
        let process = snapshot
            .processes
            .iter()
            .find(|p| p.key.pid == pid)
            .expect("disposable child is visible");
        let scope = snapshot
            .system_cpu
            .as_ref()
            .and_then(|s| s.action_scope.clone())
            .expect("scope collected");
        let request = Request {
            scope,
            key: process.key.clone(),
            name: process.name.clone(),
        };
        let mut stale = request.clone();
        stale.key.start_id += 1;
        assert!(terminate(stale).contains("identity changed"));
        let mut foreign = request.clone();
        foreign.scope.mount_namespace = "mnt:[0]".into();
        assert!(terminate(foreign).contains("identity changed"));
        let output = terminate(request);
        assert!(output.starts_with("SIGTERM accepted"), "{output}");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = crate::multiwsl::snapshot(&distro, None).unwrap();
            if !snapshot
                .processes
                .iter()
                .any(|p| p.key.pid == pid && p.key.start_id == process.key.start_id)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child still observed after SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
