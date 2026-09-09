#[cfg(unix)]
#[path = "command_unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "command_windows.rs"]
mod platform;

pub struct CommandSpec<'a> {
    pub program: &'a str,
    pub args: &'a [&'a str],
}

impl<'a> CommandSpec<'a> {
    pub fn new(program: &'a str, args: &'a [&'a str]) -> Self {
        Self { program, args }
    }
}

pub use platform::output_with_timeout;
