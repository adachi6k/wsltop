use std::ffi::c_void;
use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

type HandleValue = *mut c_void;

const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
#[derive(Default)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

unsafe extern "system" {
    fn AssignProcessToJobObject(job: HandleValue, process: HandleValue) -> i32;
    fn CloseHandle(handle: HandleValue) -> i32;
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> HandleValue;
    fn SetInformationJobObject(
        job: HandleValue,
        information_class: u32,
        information: *const c_void,
        information_length: u32,
    ) -> i32;
    fn TerminateJobObject(job: HandleValue, exit_code: u32) -> i32;
}

struct JobObject {
    handle: HandleValue,
}

impl JobObject {
    fn create() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self { handle };
        job.set_kill_on_close()?;
        Ok(job)
    }

    fn assign(&self, process: HandleValue) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.handle, process) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn set_kill_on_close(&self) -> io::Result<()> {
        let mut limits = JobObjectExtendedLimitInformation::default();
        limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                self.handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&limits as *const JobObjectExtendedLimitInformation).cast(),
                std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Runs a command with a bounded wait on Windows.
///
/// The child is assigned to a Job Object so timeout cleanup terminates the
/// whole process tree. That prevents recurring collector timeouts from leaving
/// descendants alive with inherited stdout/stderr pipes.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let job = JobObject::create()?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Err(error) = job.assign(child.as_raw_handle()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

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
            if job.terminate().is_err() {
                let _ = child.kill();
            }
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
