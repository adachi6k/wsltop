use std::io::{self, Read};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Runs a command with a bounded wait on Windows.
///
/// This first Windows implementation terminates the direct child on timeout.
/// A future implementation can attach the process to a Windows Job Object so
/// descendants are terminated with the same guarantees as the Unix process
/// group implementation.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "command stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "command stderr was not piped"))?;
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            if stdout_reader.is_finished() && stderr_reader.is_finished() {
                return collect_output(status, stdout_reader, stderr_reader);
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            // Waiting for Windows process termination is not guaranteed to be
            // bounded. Drop the join handles so inherited pipes cannot extend
            // this API's deadline; the reader threads finish when those pipes
            // eventually close.
            drop(stdout_reader);
            drop(stderr_reader);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("command exceeded {} ms", timeout.as_millis()),
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn read_all(mut stream: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn collect_output(
    status: ExitStatus,
    stdout: thread::JoinHandle<io::Result<Vec<u8>>>,
    stderr: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> io::Result<Output> {
    let stdout = stdout
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::output_with_timeout;
    use std::io::ErrorKind;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn returns_output_for_completed_command() {
        let output = output_with_timeout(
            Command::new("cmd.exe").args(["/D", "/C", "echo done"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "done");
    }

    #[test]
    fn returns_promptly_after_timeout() {
        let started = std::time::Instant::now();
        let error = output_with_timeout(
            Command::new("cmd.exe").args(["/D", "/C", "ping -n 6 127.0.0.1 > nul"]),
            Duration::from_millis(20),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn drains_output_larger_than_pipe_capacity() {
        let script = "$s = 'x' * 262144; [Console]::Out.Write($s); [Console]::Error.Write($s)";
        let output = output_with_timeout(
            Command::new("powershell.exe").args(["-NoLogo", "-NoProfile", "-Command", script]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 262_144);
        assert_eq!(output.stderr.len(), 262_144);
    }
}
