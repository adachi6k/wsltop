use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    // Put the command in its own process group so a timed-out shell cannot leave
    // descendants holding stdout/stderr open after the direct child is killed.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
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
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if let Some(status) = status {
            if stdout_reader.is_finished() && stderr_reader.is_finished() {
                return collect_output(status, stdout_reader, stderr_reader);
            }
        }
        if started.elapsed() >= timeout {
            // The process-group id is the child's pid because pre_exec created a
            // new group. Killing the group also closes pipes inherited by children.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            // Do not make the timeout itself unbounded if a descendant escaped the
            // process group or an unusual runtime kept a duplicate pipe open.
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
            Command::new("sh").args(["-c", "printf done"]),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.stdout, b"done");
    }

    #[test]
    fn terminates_timed_out_command() {
        let started = std::time::Instant::now();
        let error = output_with_timeout(
            Command::new("sh").args(["-c", "sleep 10 & wait"]),
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn timeout_includes_pipes_inherited_by_a_descendant() {
        let started = std::time::Instant::now();
        let error = output_with_timeout(
            Command::new("sh").args(["-c", "sleep 10 &"]),
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn drains_output_larger_than_pipe_capacity() {
        let output = output_with_timeout(
            Command::new("sh").args(["-c", "yes x | head -c 262144; yes y | head -c 262144 >&2"]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 262_144);
        assert_eq!(output.stderr.len(), 262_144);
    }
}
