use crate::command::CommandSpec;
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read};
use std::os::windows::io::FromRawHandle;
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::thread;
use std::time::{Duration, Instant};

type HandleValue = *mut c_void;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: i32,
}

#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    reserved2: u16,
    reserved2_data: *mut u8,
    std_input: HandleValue,
    std_output: HandleValue,
    std_error: HandleValue,
}

#[repr(C)]
#[derive(Default)]
struct ProcessInformation {
    process: HandleValue,
    thread: HandleValue,
    process_id: u32,
    thread_id: u32,
}

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
    fn CreatePipe(
        read_pipe: *mut HandleValue,
        write_pipe: *mut HandleValue,
        pipe_attributes: *mut SecurityAttributes,
        size: u32,
    ) -> i32;
    fn CreateProcessW(
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *mut SecurityAttributes,
        thread_attributes: *mut SecurityAttributes,
        inherit_handles: i32,
        creation_flags: u32,
        environment: *mut c_void,
        current_directory: *const u16,
        startup_info: *mut StartupInfoW,
        process_information: *mut ProcessInformation,
    ) -> i32;
    fn GetExitCodeProcess(process: HandleValue, exit_code: *mut u32) -> i32;
    fn ResumeThread(thread: HandleValue) -> u32;
    fn SetHandleInformation(handle: HandleValue, mask: u32, flags: u32) -> i32;
    fn SetInformationJobObject(
        job: HandleValue,
        information_class: u32,
        information: *const c_void,
        information_length: u32,
    ) -> i32;
    fn TerminateJobObject(job: HandleValue, exit_code: u32) -> i32;
    fn TerminateProcess(process: HandleValue, exit_code: u32) -> i32;
    fn WaitForSingleObject(handle: HandleValue, milliseconds: u32) -> u32;
}

struct OwnedHandle {
    handle: HandleValue,
}

impl OwnedHandle {
    fn new(handle: HandleValue) -> Self {
        Self { handle }
    }

    fn raw(&self) -> HandleValue {
        self.handle
    }

    fn take(&mut self) -> HandleValue {
        std::mem::replace(&mut self.handle, std::ptr::null_mut())
    }

    fn into_file(mut self) -> File {
        unsafe { File::from_raw_handle(self.take()) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
}

impl Pipe {
    fn create() -> io::Result<Self> {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        let mut attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            security_descriptor: std::ptr::null_mut(),
            inherit_handle: 1,
        };
        if unsafe { CreatePipe(&mut read, &mut write, &mut attributes, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let pipe = Self {
            read: OwnedHandle::new(read),
            write: OwnedHandle::new(write),
        };
        if unsafe { SetHandleInformation(pipe.read.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(pipe)
    }
}

struct JobObject {
    handle: OwnedHandle,
}

impl JobObject {
    fn create() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self {
            handle: OwnedHandle::new(handle),
        };
        job.set_kill_on_close()?;
        Ok(job)
    }

    fn assign(&self, process: HandleValue) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.handle.raw(), process) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.handle.raw(), 1) } == 0 {
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
                self.handle.raw(),
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

struct ChildProcess {
    process: OwnedHandle,
    thread: OwnedHandle,
}

impl ChildProcess {
    fn spawn_suspended(
        spec: CommandSpec<'_>,
        stdout: HandleValue,
        stderr: HandleValue,
    ) -> io::Result<Self> {
        let mut command_line = command_line(spec)
            .encode_utf16()
            .chain([0])
            .collect::<Vec<_>>();
        let mut startup = StartupInfoW {
            cb: std::mem::size_of::<StartupInfoW>() as u32,
            reserved: std::ptr::null_mut(),
            desktop: std::ptr::null_mut(),
            title: std::ptr::null_mut(),
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_count_chars: 0,
            y_count_chars: 0,
            fill_attribute: 0,
            flags: STARTF_USESTDHANDLES,
            show_window: 0,
            reserved2: 0,
            reserved2_data: std::ptr::null_mut(),
            std_input: std::ptr::null_mut(),
            std_output: stdout,
            std_error: stderr,
        };
        let mut information = ProcessInformation::default();
        if unsafe {
            CreateProcessW(
                std::ptr::null(),
                command_line.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                CREATE_NO_WINDOW | CREATE_SUSPENDED,
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut startup,
                &mut information,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            process: OwnedHandle::new(information.process),
            thread: OwnedHandle::new(information.thread),
        })
    }

    fn resume(&self) -> io::Result<()> {
        if unsafe { ResumeThread(self.thread.raw()) } == u32::MAX {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn terminate(&self) {
        unsafe {
            TerminateProcess(self.process.raw(), 1);
        }
    }

    fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        match unsafe { WaitForSingleObject(self.process.raw(), 0) } {
            WAIT_OBJECT_0 => {
                let mut exit_code = 0;
                if unsafe { GetExitCodeProcess(self.process.raw(), &mut exit_code) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(Some(ExitStatus::from_raw(exit_code)))
                }
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

/// Runs a command with a bounded wait on Windows.
///
/// The process is created suspended, assigned to a Job Object, then resumed.
/// That removes the post-spawn escape window where descendants could be created
/// before the process tree is governed by the job.
pub fn output_with_timeout(spec: CommandSpec<'_>, timeout: Duration) -> io::Result<Output> {
    let job = JobObject::create()?;
    let stdout = Pipe::create()?;
    let stderr = Pipe::create()?;
    let child = ChildProcess::spawn_suspended(spec, stdout.write.raw(), stderr.write.raw())?;
    job.assign(child.process.raw())
        .inspect_err(|_| child.terminate())?;
    child.resume().inspect_err(|_| child.terminate())?;

    drop(stdout.write);
    drop(stderr.write);
    let stdout = stdout.read.into_file();
    let stderr = stderr.read.into_file();
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
                child.terminate();
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

fn command_line(spec: CommandSpec<'_>) -> String {
    std::iter::once(spec.program)
        .chain(spec.args.iter().copied())
        .map(quote_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .bytes()
            .any(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'"'))
    {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
        } else if ch == '"' {
            quoted.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
            quoted.push('"');
            backslashes = 0;
        } else {
            quoted.extend(std::iter::repeat('\\').take(backslashes));
            quoted.push(ch);
            backslashes = 0;
        }
    }
    quoted.extend(std::iter::repeat('\\').take(backslashes * 2));
    quoted.push('"');
    quoted
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
    use super::{command_line, output_with_timeout, quote_arg};
    use crate::command::CommandSpec;
    use std::io::ErrorKind;
    use std::time::Duration;

    #[test]
    fn quotes_windows_command_line_arguments() {
        assert_eq!(quote_arg("simple"), "simple");
        assert_eq!(quote_arg("two words"), "\"two words\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r#"C:\path\"#), r#"C:\path\"#);
        assert_eq!(quote_arg(r#"C:\two words\"#), r#""C:\two words\\""#);
        assert_eq!(
            command_line(CommandSpec::new("cmd.exe", &["/D", "/C", "echo done"])),
            "cmd.exe /D /C \"echo done\""
        );
    }

    #[test]
    fn returns_output_for_completed_command() {
        let output = output_with_timeout(
            CommandSpec::new("cmd.exe", &["/D", "/C", "echo done"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "done");
    }

    #[test]
    fn returns_promptly_after_timeout() {
        let started = std::time::Instant::now();
        let error = output_with_timeout(
            CommandSpec::new("cmd.exe", &["/D", "/C", "ping -n 6 127.0.0.1 > nul"]),
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
            CommandSpec::new(
                "powershell.exe",
                &["-NoLogo", "-NoProfile", "-Command", script],
            ),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 262_144);
        assert_eq!(output.stderr.len(), 262_144);
    }
}
