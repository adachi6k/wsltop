use std::io;

#[derive(Debug, PartialEq, Eq)]
pub struct Stat<'a> {
    pub command: &'a str,
    pub user_ticks: u64,
    pub system_ticks: u64,
    pub start_ticks: u64,
    pub resident_pages: u64,
}

pub fn parse_stat<'a>(input: &'a str, source: &str) -> io::Result<Stat<'a>> {
    let open = input.find('(').ok_or_else(|| invalid_stat(source))?;
    let close = input.rfind(')').ok_or_else(|| invalid_stat(source))?;
    if close <= open {
        return Err(invalid_stat(source));
    }

    let command = &input[open + 1..close];
    let fields: Vec<&str> = input[close + 1..].split_whitespace().collect();
    if fields.len() <= 21 {
        return Err(invalid_stat(source));
    }

    Ok(Stat {
        command,
        user_ticks: parse_u64(fields[11], source)?,
        system_ticks: parse_u64(fields[12], source)?,
        start_ticks: parse_u64(fields[19], source)?,
        resident_pages: parse_i64(fields[21], source)?.max(0) as u64,
    })
}

pub fn cmdline_name(data: &[u8]) -> Option<String> {
    let first = data.split(|byte| *byte == 0).next()?;
    if first.is_empty() {
        return None;
    }
    // This is a Linux cmdline even when parsed by a Windows-native collector.
    // Use only the Linux separator so a legal backslash in a filename survives.
    let basename = first
        .rsplit(|byte| *byte == b'/')
        .find(|name| !name.is_empty() && *name != b".")
        .filter(|name| *name != b"..")
        .unwrap_or(first);
    Some(String::from_utf8_lossy(basename).into_owned())
}

fn parse_u64(value: &str, source: &str) -> io::Result<u64> {
    value.parse::<u64>().map_err(|_| invalid_stat(source))
}

fn parse_i64(value: &str, source: &str) -> io::Result<i64> {
    value.parse::<i64>().map_err(|_| invalid_stat(source))
}

fn invalid_stat(source: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("invalid {source}"))
}

#[cfg(test)]
mod tests {
    use super::{cmdline_name, parse_stat, Stat};
    use std::io::ErrorKind;

    #[test]
    fn parses_stat_fields_and_command_with_spaces() {
        let mut fields = vec!["0"; 22];
        fields[0] = "R";
        fields[11] = "120";
        fields[12] = "30";
        fields[19] = "190";
        fields[21] = "7";
        let input = format!("42 (compiler worker) {}", fields.join(" "));
        let parsed = parse_stat(&input, "/proc/42/stat").unwrap();
        let command_offset = input.find('(').unwrap() + 1;
        assert_eq!(parsed.command.as_ptr(), input[command_offset..].as_ptr());
        assert_eq!(
            parsed,
            Stat {
                command: "compiler worker",
                user_ticks: 120,
                system_ticks: 30,
                start_ticks: 190,
                resident_pages: 7,
            }
        );
    }

    #[test]
    fn clamps_negative_resident_pages() {
        let mut fields = vec!["0"; 22];
        fields[0] = "S";
        fields[21] = "-4";
        let input = format!("1 (init) {}", fields.join(" "));
        assert_eq!(parse_stat(&input, "stat").unwrap().resident_pages, 0);
    }

    #[test]
    fn rejects_short_or_invalid_stat() {
        assert_eq!(
            parse_stat("1 init", "stat").unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        let mut fields = vec!["0"; 22];
        fields[0] = "S";
        fields[11] = "nope";
        let input = format!("1 (init) {}", fields.join(" "));
        assert_eq!(
            parse_stat(&input, "stat").unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    fn extracts_first_cmdline_basename() {
        assert_eq!(
            cmdline_name(b"/usr/bin/cmake\0--build\0"),
            Some("cmake".into())
        );
        assert_eq!(cmdline_name(b""), None);
        assert_eq!(cmdline_name(b"\0"), None);
        assert_eq!(
            cmdline_name(b"/usr/bin/compiler\\worker\0--build\0"),
            Some("compiler\\worker".into())
        );
        assert_eq!(cmdline_name(b"/usr/bin/cmake/\0"), Some("cmake".into()));
        assert_eq!(cmdline_name(b"/\0"), Some("/".into()));
        assert_eq!(cmdline_name(b"/usr/bin/./\0"), Some("bin".into()));
        assert_eq!(cmdline_name(b"/usr/bin/../\0"), Some("/usr/bin/../".into()));
    }
}
