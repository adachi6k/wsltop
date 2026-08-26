use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage, WindowsApplicationUsage};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WindowsProcessMetadata {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    #[serde(default)]
    pub start_id: u64,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
}

pub type WindowsMetadata = HashMap<u32, WindowsProcessMetadata>;

pub fn group_processes(
    processes: &[ResourceUsage],
    metadata: &WindowsMetadata,
) -> Vec<WindowsApplicationUsage> {
    let rows: HashMap<u32, &ResourceUsage> = processes
        .iter()
        .filter_map(|row| row.pid.map(|pid| (pid, row)))
        .collect();
    let mut groups: BTreeMap<String, (String, Vec<ResourceUsage>)> = BTreeMap::new();

    for process in processes {
        if process.environment != EnvironmentKind::Windows || process.kind != ResourceKind::Process
        {
            continue;
        }
        let (key, display) = application_identity(process, &rows, metadata);
        groups
            .entry(key)
            .or_insert_with(|| (display, Vec::new()))
            .1
            .push(process.clone());
    }

    let mut applications: Vec<_> = groups
        .into_iter()
        .map(|(key, (display, mut processes))| {
            processes.sort_by(|left, right| right.cpu_percent.total_cmp(&left.cpu_percent));
            WindowsApplicationUsage {
                resource: ResourceUsage {
                    environment: EnvironmentKind::Windows,
                    source: None,
                    kind: ResourceKind::Application,
                    id: format!("windows-app:{key}"),
                    pid: None,
                    start_id: None,
                    ppid: None,
                    name: display,
                    args: None,
                    cpu_percent: processes.iter().map(|row| row.cpu_percent).sum(),
                    memory_bytes: processes
                        .iter()
                        .map(|row| row.memory_bytes)
                        .fold(0_u64, u64::saturating_add),
                },
                processes,
            }
        })
        .collect();
    applications.sort_by(|left, right| {
        right
            .resource
            .cpu_percent
            .total_cmp(&left.resource.cpu_percent)
            .then_with(|| left.resource.name.cmp(&right.resource.name))
    });
    applications
}

fn application_identity(
    process: &ResourceUsage,
    rows: &HashMap<u32, &ResourceUsage>,
    metadata: &WindowsMetadata,
) -> (String, String) {
    let base = executable_stem(&process.name);
    if base != "msedgewebview2" {
        return canonical_identity(&process.name);
    }

    let Some(pid) = process.pid else {
        return canonical_identity(&process.name);
    };
    let Some(process_metadata) = metadata.get(&pid).filter(|item| {
        executable_stem(&item.name) == base
            && item.start_id != 0
            && process.start_id == Some(item.start_id)
    }) else {
        return canonical_identity(&process.name);
    };
    if let Some(owner) = process_metadata
        .command_line
        .as_deref()
        .and_then(webview_exe_name)
    {
        return canonical_identity(&owner);
    }
    if let Some(owner) = process_metadata
        .executable_path
        .as_deref()
        .and_then(packaged_application)
    {
        return canonical_identity(&owner);
    }

    let mut parent = Some(process_metadata.parent_pid);
    let mut visited = HashSet::new();
    for _ in 0..8 {
        let Some(parent_pid) = parent.filter(|value| *value != 0) else {
            break;
        };
        if !visited.insert(parent_pid) {
            break;
        }
        let Some(parent_row) = rows.get(&parent_pid) else {
            break;
        };
        let Some(parent_metadata) = metadata.get(&parent_pid).filter(|item| {
            executable_stem(&item.name) == executable_stem(&parent_row.name)
                && item.start_id != 0
                && parent_row.start_id == Some(item.start_id)
        }) else {
            break;
        };
        let owner = executable_stem(&parent_row.name);
        if owner != "msedgewebview2" {
            return canonical_identity(&parent_row.name);
        }
        parent = Some(parent_metadata.parent_pid);
    }
    canonical_identity(&process.name)
}

fn executable_stem(name: &str) -> String {
    let lower = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn canonical_identity(name: &str) -> (String, String) {
    let raw = executable_stem(name);
    let (key, display) = match raw.as_str() {
        "ms-teams" | "msteams" => ("teams", "Teams"),
        "chrome" => ("chrome", "Chrome"),
        "chatgpt" | "chatgpt classic" => ("chatgpt", "ChatGPT"),
        "searchhost" => ("searchhost", "SearchHost"),
        "msedgewebview2" => ("webview2", "WebView2"),
        _ => return (raw, executable_display_name(name)),
    };
    (key.to_string(), display.to_string())
}

fn executable_display_name(name: &str) -> String {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let suffix_start = basename.len().saturating_sub(4);
    let has_exe_suffix = basename
        .as_bytes()
        .get(suffix_start..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b".exe"));
    let without_extension = if has_exe_suffix {
        basename.get(..suffix_start).unwrap_or(basename)
    } else {
        basename
    };
    without_extension.to_string()
}

fn webview_exe_name(command_line: &str) -> Option<String> {
    let marker = "--webview-exe-name=";
    let lower = command_line.to_ascii_lowercase();
    let start = lower.match_indices(marker).find_map(|(index, _)| {
        (index == 0
            || lower[..index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_whitespace()))
        .then_some(index + marker.len())
    })?;
    let value = command_line[start..].trim_start();
    let value = if let Some(value) = value.strip_prefix('"') {
        value.split('"').next()?
    } else {
        value.split_whitespace().next()?
    };
    (!value.is_empty()).then(|| value.to_string())
}

fn packaged_application(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let package = lower.split("\\windowsapps\\").nth(1)?.split('_').next()?;
    match package {
        "msteams" => Some("Teams".into()),
        "openai.chatgpt-desktop" => Some("ChatGPT".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_identity, group_processes, WindowsMetadata, WindowsProcessMetadata};
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};

    fn row(pid: u32, name: &str, cpu: f64) -> ResourceUsage {
        ResourceUsage {
            environment: EnvironmentKind::Windows,
            source: None,
            kind: ResourceKind::Process,
            id: pid.to_string(),
            pid: Some(pid),
            start_id: Some(pid as u64),
            ppid: None,
            name: name.into(),
            args: None,
            cpu_percent: cpu,
            memory_bytes: 1,
        }
    }

    fn metadata(pid: u32, parent_pid: u32, name: &str) -> WindowsProcessMetadata {
        WindowsProcessMetadata {
            pid,
            parent_pid,
            name: name.into(),
            start_id: pid as u64,
            executable_path: None,
            command_line: None,
        }
    }

    #[test]
    fn groups_teams_and_owned_webview_without_scaling() {
        let rows = vec![row(10, "ms-teams", 2.5), row(11, "msedgewebview2", 3.0)];
        let metadata = WindowsMetadata::from([
            (10, metadata(10, 1, "ms-teams.exe")),
            (11, metadata(11, 10, "msedgewebview2.exe")),
        ]);
        let groups = group_processes(&rows, &metadata);
        let teams = groups
            .iter()
            .find(|group| group.resource.name == "Teams")
            .unwrap();
        assert_eq!(teams.resource.cpu_percent, 5.5);
        assert_eq!(teams.processes.len(), 2);
    }

    #[test]
    fn search_owned_webview_does_not_join_teams() {
        let rows = vec![
            row(10, "ms-teams", 1.0),
            row(20, "SearchHost", 1.0),
            row(21, "msedgewebview2", 1.0),
        ];
        let metadata = WindowsMetadata::from([
            (10, metadata(10, 1, "ms-teams.exe")),
            (20, metadata(20, 1, "SearchHost.exe")),
            (21, metadata(21, 20, "msedgewebview2.exe")),
        ]);
        let groups = group_processes(&rows, &metadata);
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "Teams")
                .unwrap()
                .processes
                .len(),
            1
        );
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "SearchHost")
                .unwrap()
                .processes
                .len(),
            2
        );
    }

    #[test]
    fn ambiguous_webview_remains_conservative() {
        let groups = group_processes(&[row(21, "msedgewebview2", 1.0)], &WindowsMetadata::new());
        assert_eq!(groups[0].resource.name, "WebView2");
    }

    #[test]
    fn stale_webview_metadata_is_not_trusted_after_pid_reuse() {
        let mut stale = metadata(21, 10, "old-process.exe");
        stale.command_line = Some("--webview-exe-name=ms-teams.exe".into());
        let groups = group_processes(
            &[row(21, "msedgewebview2", 1.0)],
            &WindowsMetadata::from([(21, stale)]),
        );
        assert_eq!(groups[0].resource.name, "WebView2");
    }

    #[test]
    fn same_executable_metadata_is_not_trusted_across_process_generations() {
        let mut stale = metadata(21, 10, "msedgewebview2.exe");
        stale.start_id = 20;
        stale.command_line = Some("--webview-exe-name=ms-teams.exe".into());
        let groups = group_processes(
            &[row(21, "msedgewebview2", 1.0)],
            &WindowsMetadata::from([(21, stale)]),
        );
        assert_eq!(groups[0].resource.name, "WebView2");
    }

    #[test]
    fn embedded_webview_option_marker_is_not_ownership_evidence() {
        let mut current = metadata(21, 0, "msedgewebview2.exe");
        current.command_line =
            Some(r"msedgewebview2.exe --log-path=C:\--webview-exe-name=ms-teams.exe".into());
        let groups = group_processes(
            &[row(21, "msedgewebview2", 1.0)],
            &WindowsMetadata::from([(21, current)]),
        );
        assert_eq!(groups[0].resource.name, "WebView2");
    }

    #[test]
    fn similarly_named_windowsapps_directory_is_not_package_evidence() {
        let mut current = metadata(21, 0, "msedgewebview2.exe");
        current.executable_path =
            Some(r"C:\NotWindowsApps\MSTeams_1.0_x64\msedgewebview2.exe".into());
        let groups = group_processes(
            &[row(21, "msedgewebview2", 1.0)],
            &WindowsMetadata::from([(21, current)]),
        );
        assert_eq!(groups[0].resource.name, "WebView2");
    }

    #[test]
    fn actual_windowsapps_component_is_package_evidence() {
        let mut current = metadata(21, 0, "msedgewebview2.exe");
        current.executable_path =
            Some(r"C:\Program Files\WindowsApps\MSTeams_1.0_x64\msedgewebview2.exe".into());
        let groups = group_processes(
            &[row(21, "msedgewebview2", 1.0)],
            &WindowsMetadata::from([(21, current)]),
        );
        assert_eq!(groups[0].resource.name, "Teams");
    }

    #[test]
    fn parent_without_matching_metadata_is_not_trusted() {
        let rows = vec![row(10, "ms-teams", 1.0), row(21, "msedgewebview2", 1.0)];
        let child = metadata(21, 10, "msedgewebview2.exe");
        let groups = group_processes(&rows, &WindowsMetadata::from([(21, child)]));
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "Teams")
                .unwrap()
                .processes
                .len(),
            1
        );
        assert!(groups.iter().any(|group| group.resource.name == "WebView2"));
    }

    #[test]
    fn default_identity_hides_path_and_case_insensitive_exe_suffix() {
        assert_eq!(
            canonical_identity(r"C:\Program Files\Example App\Example.EXE"),
            ("example".into(), "Example".into())
        );
    }

    #[test]
    fn unmapped_application_preserves_display_casing_while_grouping_case_insensitively() {
        let groups = group_processes(
            &[row(1, "Code.exe", 1.0), row(2, "CODE.EXE", 2.0)],
            &WindowsMetadata::new(),
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].resource.name, "Code");
        assert_eq!(groups[0].resource.cpu_percent, 3.0);
    }

    #[test]
    fn webview_preserves_unmapped_parent_display_casing() {
        let rows = vec![
            row(10, "CustomHost.EXE", 1.0),
            row(21, "msedgewebview2", 1.0),
        ];
        let metadata = WindowsMetadata::from([
            (10, metadata(10, 1, "CustomHost.EXE")),
            (21, metadata(21, 10, "msedgewebview2.exe")),
        ]);
        let groups = group_processes(&rows, &metadata);
        let host = groups
            .iter()
            .find(|group| group.resource.name == "CustomHost")
            .unwrap();
        assert_eq!(host.processes.len(), 2);
    }

    #[test]
    fn command_line_owner_and_ordinary_apps_group() {
        let rows = vec![
            row(1, "chrome", 1.0),
            row(2, "chrome.exe", 2.0),
            row(3, "msedgewebview2", 3.0),
            row(4, "ChatGPT", 1.0),
            row(5, "ChatGPT Classic", 2.0),
        ];
        let mut webview = metadata(3, 0, "msedgewebview2.exe");
        webview.command_line = Some("--webview-exe-name=ms-teams.exe".into());
        let groups = group_processes(&rows, &WindowsMetadata::from([(3, webview)]));
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "Chrome")
                .unwrap()
                .resource
                .cpu_percent,
            3.0
        );
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "Teams")
                .unwrap()
                .processes
                .len(),
            1
        );
        assert_eq!(
            groups
                .iter()
                .find(|group| group.resource.name == "ChatGPT")
                .unwrap()
                .processes
                .len(),
            2
        );
    }
}
