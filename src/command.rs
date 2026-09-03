#[cfg(unix)]
#[path = "command_unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "command_windows.rs"]
mod platform;

pub use platform::output_with_timeout;
