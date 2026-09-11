use crate::model::EnvironmentKind;
use crate::monitor::MonitorSnapshot;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HeaderMode {
    Classic,
    #[default]
    Compact,
}

impl HeaderMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "classic" => Ok(Self::Classic),
            "compact" => Ok(Self::Compact),
            _ => Err(format!(
                "invalid header {value:?}; expected classic or compact"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "invalid color {value:?}; expected auto, always or never"
            )),
        }
    }

    pub fn enabled(self) -> bool {
        self == Self::Always
            || (self == Self::Auto
                && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                && std::env::var_os("TERM").is_none_or(|value| value != "dumb"))
    }
}

pub fn separator_style(colors: bool) -> Style {
    if colors {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    }
}

pub fn environment_style(environment: EnvironmentKind, colors: bool) -> Style {
    if !colors {
        return Style::default();
    }
    Style::default().fg(match environment {
        EnvironmentKind::Windows => Color::Blue,
        EnvironmentKind::Wsl => Color::Green,
        EnvironmentKind::WslContainer => Color::Magenta,
        EnvironmentKind::Docker => Color::Cyan,
    })
}

fn memory_label(snapshot: Option<&MonitorSnapshot>) -> String {
    snapshot
        .and_then(|snapshot| snapshot.host_memory)
        .and_then(|memory| memory.used_bytes().map(|used| (used, memory.total_bytes)))
        .map_or_else(
            || "N/A".into(),
            |(used, total)| {
                let mut unit = 1073741824.0;
                let mut suffix = 'G';
                for next in ['T', 'P', 'E'] {
                    if total as f64 / unit < 1024.0 {
                        break;
                    }
                    unit *= 1024.0;
                    suffix = next;
                }
                let precision = usize::from((total as f64 / unit * 10.0).round() < 1000.0);
                format!(
                    "{:.precision$}/{:.precision$}{suffix}",
                    used as f64 / unit,
                    total as f64 / unit
                )
            },
        )
}

// Reserve six cells per observation, including its unit. Use fewer decimals or
// a larger unit only when necessary, rather than moving every following column.
fn observation_memory(bytes: u64) -> String {
    let original = crate::render::format_bytes(bytes);
    if original.len() <= 6 {
        return original;
    }
    let mut value = bytes as f64;
    let mut suffix = 'B';
    for next in ['K', 'M', 'G', 'T', 'P', 'E'] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        suffix = next;
    }
    for precision in (0..=2).rev() {
        let text = format!("{value:.precision$}{suffix}");
        if text.len() <= 6 {
            return text;
        }
    }
    unreachable!("u64 bytes fit in six cells with E units")
}

fn observation_cpu(value: f64) -> String {
    let text = format!("{value:.1}%");
    if text.len() <= 6 {
        text
    } else {
        format!("{value:.0e}%")
    }
}

pub fn compact(
    snapshot: Option<&MonitorSnapshot>,
    width: u16,
    height: u16,
    colors: bool,
    wsl_only: bool,
    interval: Duration,
    now: Instant,
) -> Vec<Line<'static>> {
    let cpu = snapshot
        .and_then(|snapshot| snapshot.host_cpu_percent)
        .map_or_else(|| "N/A".into(), |value| format!("{value:.1}%"));
    let ram = memory_label(snapshot);
    if height < 2 {
        return vec![fit(format!("CPU {cpu} RAM {ram}"), &[], width, colors)];
    }
    let cpu_total = format!("CPU {cpu:<10}");
    let ram_total = format!("RAM {ram:<10}");
    if width < 80 {
        return vec![
            fit(
                if width < 14 {
                    format!("CPU {}", cpu.trim())
                } else {
                    cpu_total
                },
                &[],
                width,
                colors,
            ),
            fit(
                if width < 14 {
                    format!("RAM {ram}")
                } else {
                    ram_total
                },
                &[],
                width,
                colors,
            ),
        ];
    }
    let labels = ["Win", "WSL", "WSLC", "Docker"];
    let mut cpu_chips = Vec::new();
    let mut ram_chips = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let usage = snapshot.and_then(|snapshot| snapshot.environment_summary.0[index]);
        let cpu_value =
            usage.map_or_else(|| "N/A".into(), |usage| observation_cpu(usage.cpu_percent));
        let ram_value = usage.map_or_else(
            || "N/A".into(),
            |usage| observation_memory(usage.memory_bytes),
        );
        let style = environment_style(crate::summary::ENVIRONMENTS[index], colors);
        cpu_chips.push(Span::styled(format!("{label} {cpu_value:>6}"), style));
        ram_chips.push(Span::styled(format!("{label} {ram_value:>6}"), style));
    }
    let mut cpu_total = cpu_total;
    let mut ram_total = ram_total;
    // Discrete width tiers keep columns stable across readings and N/A states.
    let columns = if width >= 120 { 23 } else { 15 };
    if !wsl_only {
        let empty = crate::history::HostHistory::new(now, interval);
        let history = snapshot.map_or(&empty, |snapshot| &snapshot.host_history);
        let ascii = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
        cpu_total.push(' ');
        cpu_total.push_str(&history.cpu.sparkline(columns, now, ascii));
        ram_total.push(' ');
        ram_total.push_str(&history.memory.sparkline(columns, now, ascii));
    }
    vec![
        fit(cpu_total, &cpu_chips, width, colors),
        fit(ram_total, &ram_chips, width, colors),
    ]
}

// Labels are ASCII and sparkline glyphs occupy one cell. Never split a chip or wrap.
fn fit(total: String, chips: &[Span<'static>], width: u16, colors: bool) -> Line<'static> {
    let width = usize::from(width);
    if total.chars().count() > width {
        return Line::raw(if width >= 3 {
            format!("{}...", total.chars().take(width - 3).collect::<String>())
        } else {
            ".".repeat(width)
        });
    }
    let mut used = total.chars().count();
    let mut spans = vec![Span::raw(total)];
    for (index, chip) in chips.iter().enumerate() {
        let prefix = if index == 0 {
            " | "
        } else if width >= 120 {
            "   "
        } else {
            " "
        };
        let reserve = if index + 1 < chips.len() { 4 } else { 0 };
        if used + prefix.len() + chip.content.len() + reserve > width {
            if used + 4 <= width {
                spans.push(Span::raw(" ..."));
            }
            break;
        }
        spans.push(Span::styled(
            prefix,
            if index == 0 {
                separator_style(colors)
            } else {
                Style::default()
            },
        ));
        spans.push(chip.clone());
        used += prefix.len() + chip.content.len();
    }
    Line::from(spans)
}

/// Style only known environment labels produced by the text renderer, never command text.
pub fn resource_line(line: &str, colors: bool) -> Line<'static> {
    let labels = [
        ("Windows", EnvironmentKind::Windows),
        ("WSLC", EnvironmentKind::WslContainer),
        ("WSL", EnvironmentKind::Wsl),
        ("Docker", EnvironmentKind::Docker),
    ];
    for (label, environment) in labels {
        if line
            .strip_prefix(label)
            .is_some_and(|rest| rest.starts_with(' '))
        {
            return Line::from(vec![
                Span::styled(label, environment_style(environment, colors)),
                Span::raw(line[label.len()..].to_owned()),
            ]);
        }
    }
    Line::raw(line.to_owned())
}

pub const HELP: &str = "Summary: independent observations, NOT an additive host breakdown.\n\n\
Host CPU: all Windows logical CPUs together = 100%.\n\
Host RAM: physical total minus available; G/M/K use powers of 1024.\n\
History: left is older, right is now; fixed 0-100% scale for CPU and RAM.\n\
Each column spans the configured refresh interval, also shown in the footer.\n\
History: 23 slots at 120+ columns, 15 at 80-119; hidden below 80.\n\
At the default 3s interval these cover 69s and 45s respectively.\n\
All columns shift left together; waiting slots hold the previous value.\n\
Blank: before first sample. '!': failed/unavailable until recovery.\n\
TERM=dumb uses ASCII levels. No host history in WSL-only.\n\
Win: observed Windows processes, excluding WSL/WSLC VM hosts.\n\
WSL: processes in the primary and collected additional distributions.\n\
WSL observations may include workloads also shown under Docker.\n\
WSLC / Docker: container totals, excluding their process detail rows.\n\
RAM observations: Win working sets; WSL RSS; container CLI memory.\n\
Shared pages and overlapping environments prevent adding these values.\n\
N/A: disabled, warming up, unavailable or incomplete collection.\n\
With --wsl-only, observed CPU uses WSL-visible CPUs = 100%.\n\
Filters, limits, sorting and row CPU scale do not change the summary.\n\n\
? close help | t tree/flat | c/m/n sort CPU/memory/name | r reverse\n\
i infrastructure | h VM hosts | 0 zero rows | arrows/Pg scroll\n\
--header classic restores the one-line header; --color never disables colors.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_heights_and_monochrome_do_not_lose_totals_or_wrap() {
        for width in [0, 1, 2, 10, 40, 60, 80, 120] {
            for height in [1, 2] {
                let lines = compact(
                    None,
                    width,
                    height,
                    false,
                    false,
                    Duration::from_secs(3),
                    Instant::now(),
                );
                assert_eq!(lines.len(), usize::from(height));
                assert!(lines.iter().all(|line| line.width() <= usize::from(width)));
                assert!(lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .all(|span| span.style == Style::default()));
            }
        }
        let lines = compact(
            None,
            80,
            2,
            false,
            false,
            Duration::from_secs(3),
            Instant::now(),
        );
        assert!(lines[0].to_string().contains("Docker    N/A"));
        assert!(lines[1].to_string().starts_with("RAM N/A       "));
        assert!(!compact(
            None,
            40,
            2,
            false,
            false,
            Duration::from_secs(3),
            Instant::now()
        )[0]
        .to_string()
        .contains("Win"));
    }

    #[test]
    fn summary_width_tiers_keep_equal_graphs_and_hide_breakdown_below_80() {
        let now = Instant::now();
        for width in [40, 79, 80, 119, 120, 160] {
            let lines = compact(None, width, 2, false, false, Duration::from_secs(3), now);
            assert_eq!(lines.len(), 2);
            assert!(lines[0].to_string().starts_with("CPU "));
            assert!(lines[1].to_string().starts_with("RAM "));
            let total_and_graph = if width >= 120 {
                38
            } else if width >= 80 {
                30
            } else {
                14
            };
            for line in &lines {
                assert_eq!(line.spans[0].width(), total_and_graph);
                assert!(line.width() <= usize::from(width));
                assert!(!line.to_string().contains("Host"));
                assert!(!line.to_string().contains("WSL*"));
                assert!(!line.to_string().contains("obs"));
                assert_eq!(line.to_string().contains("Docker"), width >= 80);
            }
        }
    }

    #[test]
    fn colors_apply_to_labels_only() {
        let line = resource_line("Docker  container 123 Docker-command", true);
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[1].style, Style::default());
        assert_eq!(
            resource_line("Docker-command", true).spans[0].style,
            Style::default()
        );
    }

    #[test]
    fn summary_divider_uses_separator_style_without_dimming_values() {
        for colors in [false, true] {
            for width in [80, 120, 160] {
                let lines = compact(
                    None,
                    width,
                    2,
                    colors,
                    false,
                    Duration::from_secs(3),
                    Instant::now(),
                );
                for line in lines {
                    assert_eq!(line.spans[1].content, " | ");
                    assert_eq!(line.spans[1].style, super::separator_style(colors));
                    for (index, span) in line.spans.iter().enumerate() {
                        if index != 1 {
                            assert!(!span
                                .style
                                .add_modifier
                                .contains(ratatui::style::Modifier::DIM));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn renders_host_endpoints_and_observations_without_rescaling_or_stacking() {
        use crate::model::HostMemory;
        use crate::summary::{EnvironmentSummary, Usage};
        let config = crate::monitor::MonitorConfig {
            sort: Default::default(),
            interval: std::time::Duration::from_secs(3),
            limit: 1,
            show_wsl_host: false,
            wsl_only: false,
            no_wslc: false,
            no_docker: false,
            hide_infra: true,
            show_container_processes: false,
            container_process_limit: 1,
            collect_windows_applications: false,
        };
        let tree = crate::attribution::build_tree_with_docker(16, &[], &[], &[], &[]);
        let mut snapshot = MonitorSnapshot::from_collected(vec![], tree, vec![], &config);
        snapshot.environment_summary = EnvironmentSummary(
            [Some(Usage {
                cpu_percent: 12.5,
                memory_bytes: 1073741824,
            }); 4],
        );
        for (cpu, available, ram) in [(0.0, 34359738368, "0.0/32.0G"), (100.0, 0, "32.0/32.0G")] {
            snapshot.host_cpu_percent = Some(cpu);
            snapshot.host_memory = Some(HostMemory {
                total_bytes: 34359738368,
                available_bytes: available,
            });
            let now = Instant::now();
            snapshot.host_history.cpu.record(now, Some(cpu));
            snapshot.host_history.memory.record(now, Some(cpu));
            let lines = compact(
                Some(&snapshot),
                80,
                2,
                true,
                false,
                Duration::from_secs(3),
                now,
            );
            assert!(lines.iter().all(|line| line.width() <= 80));
            assert!(lines[0].to_string().contains(&format!("{cpu:.1}%")));
            assert!(lines[0].to_string().contains("WSL  12.5%"));
            assert!(lines[1].to_string().contains(ram));
            assert!(lines[1].to_string().contains("Docker  1.00G"));
            let ascii = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
            let expected = if cpu == 0.0 {
                if ascii {
                    '_'
                } else {
                    '▁'
                }
            } else if ascii {
                '#'
            } else {
                '█'
            };
            assert!(lines[0].spans[0].content.contains(expected));
            assert!(lines[1].spans[0].content.contains(expected));
            for width in [40, 60, 70, 75, 80, 120] {
                let narrow = compact(
                    Some(&snapshot),
                    width,
                    2,
                    false,
                    false,
                    Duration::from_secs(3),
                    now,
                );
                assert!(narrow.iter().all(|line| line.width() <= usize::from(width)));
                if width <= 60 {
                    assert!(!narrow[0].spans[0].content.contains(expected));
                }
            }
            let small = compact(
                Some(&snapshot),
                80,
                1,
                false,
                false,
                Duration::from_secs(3),
                now,
            );
            assert_eq!(small.len(), 1);
            assert!(!small[0].to_string().contains(expected));
            let guest = compact(
                Some(&snapshot),
                80,
                2,
                false,
                true,
                Duration::from_secs(3),
                now,
            );
            assert!(!guest[0].spans[0].content.contains(expected));
        }

        // Missing values, changes in digit count and larger memory units must not
        // move the history or any environment column in either row.
        let positions = |line: &Line<'_>| {
            let text = line.to_string();
            ["|", "Win", "WSL", "WSLC", "Docker"]
                .map(|label| text[..text.find(label).unwrap()].chars().count())
        };
        let now = Instant::now();
        for width in [80, 100, 119, 120, 160] {
            let empty = compact(None, width, 2, false, false, Duration::from_secs(3), now);
            let expected = positions(&empty[0]);
            assert_eq!(positions(&empty[1]), expected);
            for (cpu, bytes) in [
                (0.0_f64, 0),
                (9.9, 1023),
                (99.9, 10 * 1073741824),
                (100.0, 100 * 1073741824),
                (12345.0, u64::MAX),
            ] {
                snapshot.host_cpu_percent = Some(cpu.min(100.0));
                snapshot.host_memory = Some(HostMemory {
                    total_bytes: bytes.max(1),
                    available_bytes: bytes / 2,
                });
                snapshot.environment_summary = EnvironmentSummary(
                    [Some(Usage {
                        cpu_percent: cpu,
                        memory_bytes: bytes,
                    }); 4],
                );
                let lines = compact(
                    Some(&snapshot),
                    width,
                    2,
                    false,
                    false,
                    Duration::from_secs(3),
                    now,
                );
                for line in &lines {
                    assert_eq!(positions(line), expected, "{}", line);
                    assert!(line.width() <= usize::from(width));
                    // Labels and first value characters start at the same cells;
                    // padding belongs after the total, before the fixed history.
                    assert!(!line.to_string().chars().nth(4).unwrap().is_whitespace());
                    let gap = if width >= 120 { "   " } else { " " };
                    assert_eq!(line.spans[3].content, gap);
                    assert_eq!(line.spans[5].content, gap);
                    assert_eq!(line.spans[7].content, gap);
                }
                assert_eq!(lines[0].spans[0].width(), empty[0].spans[0].width());
                assert_eq!(lines[1].spans[0].width(), empty[1].spans[0].width());
            }
        }
    }
}
