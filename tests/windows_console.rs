#![cfg(windows)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy)]
struct KeyRecord {
    kind: u16,
    padding: u16,
    down: i32,
    repeat: u16,
    virtual_key: u16,
    scan: u16,
    character: u16,
    control: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
    fn AllocConsole() -> i32;
    fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    fn WriteConsoleInputW(
        handle: *mut c_void,
        records: *const KeyRecord,
        count: u32,
        written: *mut u32,
    ) -> i32;
}

fn mode(file: &File) -> io::Result<u32> {
    let mut mode = 0;
    if unsafe { GetConsoleMode(file.as_raw_handle(), &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    if unsafe { SetConsoleMode(file.as_raw_handle(), mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait(child: &mut Child) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "console child did not exit",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn restores_console_modes_on_normal_and_startup_error_exits() {
    for scenario in ["q-compact", "esc-classic", "startup-error"] {
        let report = std::env::temp_dir().join(format!(
            "wsltop-console-{}-{scenario}.txt",
            std::process::id()
        ));
        let mut child = Reap(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native_console_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("WSLTOP_CONSOLE_SCENARIO", scenario)
                .env("WSLTOP_CONSOLE_REPORT", &report)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let status = wait(&mut child.0).unwrap();
        let result = std::fs::read_to_string(&report).unwrap_or_else(|error| error.to_string());
        let _ = std::fs::remove_file(&report);
        assert!(status.success(), "{scenario}: {result}");
        assert!(result.starts_with("PASS"), "{scenario}: {result}");
        println!("{scenario}: {result}");
    }
}

#[test]
#[ignore = "launched in an isolated process by the parent regression test"]
fn native_console_child() {
    let scenario = std::env::var("WSLTOP_CONSOLE_SCENARIO").expect("parent supplies scenario");
    let report = std::env::var_os("WSLTOP_CONSOLE_REPORT").expect("parent supplies report");
    let result = std::panic::catch_unwind(|| console_case(&scenario));
    let description = match &result {
        Ok(Ok(value)) => value.clone(),
        Ok(Err(error)) => format!("FAIL: {error}"),
        Err(panic) => format!(
            "FAIL: {}",
            panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("panic")
        ),
    };
    std::fs::write(report, description).unwrap();
    assert!(matches!(result, Ok(Ok(_))));
}

fn console_case(scenario: &str) -> io::Result<String> {
    // Never change the test runner's or developer's console. This helper owns a
    // separate real Win32 console, even when CI itself has redirected stdio.
    unsafe {
        FreeConsole();
    }
    if unsafe { AllocConsole() } == 0 {
        return Err(io::Error::last_os_error());
    }
    let input = OpenOptions::new().read(true).write(true).open("CONIN$")?;
    let output = OpenOptions::new().read(true).write(true).open("CONOUT$")?;
    let failing = scenario == "startup-error";
    // Echo is deliberately disabled initially: merely calling disable_raw_mode
    // would enable it and fail this regression even on a plain CI console.
    set_mode(&input, 0x01f3)?;
    set_mode(&output, if failing { 0x0007 } else { 0x0003 })?;
    let before = (mode(&input)?, mode(&output)?);
    let mut child_output = OpenOptions::new()
        .read(true)
        .write(!failing)
        .open("CONOUT$")?;
    assert_eq!(mode(&child_output)?, before.1);
    if failing {
        // GetConsoleMode succeeds, but entering the alternate screen cannot be
        // written. TerminalGuard has already enabled raw input at this point.
        assert!(child_output.write_all(b"x").is_err());
    }
    let executable = std::env::var_os("WSLTOP_CONSOLE_TEST_EXE")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsltop").into());
    let mut child = Reap(
        Command::new(executable)
            .args([
                "--interactive",
                "--wsl-only",
                "--no-docker",
                "--header",
                if scenario == "esc-classic" {
                    "classic"
                } else {
                    "compact"
                },
            ])
            .stdin(input.try_clone()?)
            .stdout(child_output)
            .stderr(output.try_clone()?)
            .spawn()?,
    );
    if !failing {
        let deadline = Instant::now() + Duration::from_secs(20);
        while mode(&input)? & 7 != 0 {
            assert!(child.0.try_wait()?.is_none(), "TUI exited before raw mode");
            assert!(Instant::now() < deadline, "TUI did not enter raw mode");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(mode(&input)?, before.0);
        let escape = scenario == "esc-classic";
        let down = KeyRecord {
            kind: 1,
            padding: 0,
            down: 1,
            repeat: 1,
            virtual_key: if escape { 27 } else { 0x51 },
            scan: 0,
            character: if escape { 27 } else { b'q' as u16 },
            control: 0,
        };
        let records = [down, KeyRecord { down: 0, ..down }];
        let mut written = 0;
        if unsafe { WriteConsoleInputW(input.as_raw_handle(), records.as_ptr(), 2, &mut written) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        assert_eq!(written, 2);
    }
    let status = wait(&mut child.0)?;
    assert_eq!(status.success(), !failing, "unexpected exit: {status}");
    let after = (mode(&input)?, mode(&output)?);
    assert_eq!(after, before, "console modes were not restored");
    Ok(format!(
        "PASS before={before:?} after={after:?} exit={status}"
    ))
}
