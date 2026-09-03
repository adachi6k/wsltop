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
            let _ = child.wait();
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
