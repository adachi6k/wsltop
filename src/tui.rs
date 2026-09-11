use crate::header::{self, separator_style, ColorMode, HeaderMode};
use crate::monitor::{MonitorConfig, MonitorSnapshot};
use crate::query::{ResourceQuery, SortKey, SortOrder};
use crate::render;
use crate::render::CpuScale;
use crate::stream;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Terminal;
use std::error::Error;
use std::io::{self, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn run(
    config: MonitorConfig,
    distro: Option<String>,
    initial_tree: bool,
    cpu_scale: CpuScale,
    header_mode: HeaderMode,
    color_mode: ColorMode,
) -> Result<(), Box<dyn Error>> {
    let interval = config.interval;
    let colors = color_mode.enabled();
    // Crossterm also caches NO_COLOR; keep its global switch consistent with our
    // explicit --color policy, including --color always overriding NO_COLOR.
    crossterm::style::force_color_output(colors);
    let mut terminal = TerminalGuard::new()?;
    let mut state = State::from_config(&config, initial_tree, cpu_scale);
    state.colors = colors;
    let wsl_only = config.wsl_only;
    let worker = SamplingWorker::start(config, distro, initial_tree);

    loop {
        for result in worker.receiver.try_iter() {
            state.apply_sample(result);
        }
        terminal
            .terminal
            .draw(|frame| draw_ui(frame, &mut state, header_mode, wsl_only, interval))?;

        if event::poll(Duration::from_millis(100))? {
            if let Some(code) = actionable_key(event::read()?) {
                if state.key(code) {
                    break;
                }
                if matches!(
                    code,
                    KeyCode::Char('t' | 'i' | 'h' | '0' | 'c' | 'm' | 'n' | 'r')
                ) {
                    state.rebuild_lines();
                    worker.set_details(state.tree);
                }
            }
        }
    }
    Ok(())
}

fn layout_areas(area: Rect, mode: HeaderMode) -> [Rect; 4] {
    let summary_height = if mode == HeaderMode::Compact && area.height >= 4 {
        2
    } else {
        1
    };
    Layout::vertical([
        Constraint::Length(summary_height),
        Constraint::Length(u16::from(area.height >= summary_height + 3)),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area)
}

fn draw_ui(
    frame: &mut ratatui::Frame<'_>,
    state: &mut State,
    mode: HeaderMode,
    wsl_only: bool,
    interval: Duration,
) {
    let [summary, separator, table, footer] = layout_areas(frame.area(), mode);
    let summary_lines = if mode == HeaderMode::Classic {
        vec![Line::raw(state.classic_header(summary.width, interval))]
    } else {
        header::compact(
            state.snapshot.as_ref(),
            summary.width,
            summary.height,
            state.colors,
            wsl_only,
            interval,
            std::time::Instant::now(),
        )
    };
    let structural_lines = if state.tree {
        &[][..]
    } else {
        &state.lines[..]
    };
    let separator_width =
        summary_separator_width(separator.width, &summary_lines, structural_lines);
    frame.render_widget(Paragraph::new(summary_lines), summary);
    let ascii = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    frame.render_widget(
        Paragraph::new(summary_separator(separator_width, state.colors, ascii)),
        separator,
    );
    if state.help {
        let paragraph = Paragraph::new(state.help_text(interval)).wrap(Wrap { trim: false });
        let help_lines = paragraph.line_count(table.width);
        state.help_scroll = state.help_scroll.min(
            help_lines
                .saturating_sub(usize::from(table.height))
                .min(usize::from(u16::MAX)) as u16,
        );
        frame.render_widget(paragraph.scroll((state.help_scroll, 0)), table);
    } else {
        let height = usize::from(table.height);
        state.clamp_scroll(height);
        frame.render_widget(
            Paragraph::new(
                state
                    .lines
                    .iter()
                    .skip(state.scroll)
                    .take(height)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            table,
        );
    }
    frame.render_widget(Paragraph::new(state.footer(footer.width, interval)), footer);
}

fn summary_separator_width(available: u16, summary: &[Line<'_>], table: &[Line<'_>]) -> u16 {
    // Match the summary or table headings/rule, whichever is wider. Long command
    // rows and scrolling must not stretch this structural separator.
    summary
        .iter()
        .chain(table.iter().take(2))
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .min(usize::from(available)) as u16
}

fn summary_separator(width: u16, colors: bool, ascii: bool) -> Line<'static> {
    Line::styled(
        if ascii { "-" } else { "─" }.repeat(usize::from(width)),
        separator_style(colors),
    )
}

fn fit_text(text: &str, width: usize) -> String {
    if Line::raw(text).width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    for ch in text.chars() {
        let mut candidate = result.clone();
        candidate.push(ch);
        if Line::raw(candidate.as_str()).width() + 1 > width {
            break;
        }
        result.push(ch);
    }
    result.push('…');
    result
}

fn actionable_key(event: Event) -> Option<KeyCode> {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => Some(key.code),
        _ => None,
    }
}

fn host_cpu_label(snapshot: Option<&MonitorSnapshot>) -> String {
    snapshot
        .and_then(|snapshot| snapshot.host_cpu_percent)
        .map_or_else(|| "N/A".to_string(), |cpu| format!("{cpu:.1}%"))
}

#[derive(Default)]
struct State {
    colors: bool,
    help: bool,
    help_scroll: u16,
    query: ResourceQuery,
    lines: Vec<Line<'static>>,
    scroll: usize,
    tree: bool,
    hide_infra: bool,
    show_hosts: bool,
    status: String,
    hide_zero: bool,
    snapshot: Option<MonitorSnapshot>,
    cpu_scale: CpuScale,
}

impl State {
    fn classic_header(&self, width: u16, interval: Duration) -> String {
        fit_text(
            &format!(
                " {} | Host CPU {} | CPU {} | sort {} {} | interval {}ms",
                if self.tree { "tree" } else { "flat" },
                host_cpu_label(self.snapshot.as_ref()),
                self.cpu_scale.label(),
                self.query.sort.key.label(),
                self.query.sort.order.label(),
                interval.as_millis()
            ),
            usize::from(width),
        )
    }

    fn footer(&self, width: u16, interval: Duration) -> String {
        let width = usize::from(width);
        let view = if self.tree { "tree" } else { "flat" };
        let key = match self.query.sort.key {
            SortKey::Memory => "mem",
            key => key.label(),
        };
        let arrow = if self.query.sort.order == SortOrder::Desc {
            '↓'
        } else {
            '↑'
        };
        let scale = if self.cpu_scale == CpuScale::Core {
            "core"
        } else {
            "host"
        };
        // Keep scale ahead of interval and optional hints when space is limited.
        let mut text = [
            format!(
                "[{view} {key}{arrow} {scale} {:.1}s]  q quit",
                interval.as_secs_f64()
            ),
            format!("[{view} {key}{arrow} {scale}]  q quit"),
            format!("[{view} {key}{arrow}]  q quit"),
        ]
        .into_iter()
        .find(|text| Line::raw(text.as_str()).width() <= width)
        .unwrap_or_else(|| format!("{view} {key}{arrow} q quit"));
        if Line::raw(text.as_str()).width() > width {
            text = format!("{view} {key}{arrow} q quit");
            if Line::raw(text.as_str()).width() <= width {
                return text;
            }
            if width <= 6 {
                return fit_text("q quit", width);
            }
            return format!(
                "{} q quit",
                fit_text(&format!("{view} {key}{arrow}"), width - 7)
            );
        }
        let status = self
            .status
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect::<String>();
        let separator = if width >= 120 { "  " } else { " " };
        let mut items = Vec::new();
        if !status.is_empty() {
            items.push("!".to_owned());
        }
        items.push(if self.help { "? close" } else { "? help" }.to_owned());
        if width >= 80 {
            items.extend([
                "t tree".to_owned(),
                format!("i infra:{}", on_off(!self.hide_infra)),
                format!("h hosts:{}", on_off(self.show_hosts)),
                format!("0 zero:{}", on_off(!self.hide_zero)),
            ]);
        }
        for item in items {
            let candidate = format!("{text}{separator}{item}");
            if Line::raw(candidate.as_str()).width() <= width {
                text = candidate;
            } else {
                break;
            }
        }
        if width >= 120 && !status.is_empty() {
            let available = width.saturating_sub(Line::raw(text.as_str()).width() + 3);
            if available >= 5 {
                text.push_str(" | ");
                text.push_str(&fit_text(&status, available));
            }
        }
        text
    }

    fn help_text(&self, interval: Duration) -> String {
        let mut text = format!(
            "Help (? / Esc close, arrows / Pg scroll)\nRefresh: {:.1}s | Row CPU scale: {}\nSort: {} {}\n",
            interval.as_secs_f64(),
            self.cpu_scale.label(),
            self.query.sort.key.label(),
            self.query.sort.order.label()
        );
        if !self.status.is_empty() {
            text.push_str(&format!("Status: {}\n", self.status));
        }
        text.push('\n');
        text.push_str(header::HELP);
        text
    }

    fn from_config(config: &MonitorConfig, tree: bool, cpu_scale: CpuScale) -> Self {
        Self {
            query: config.query(),
            tree,
            hide_infra: config.hide_infra,
            show_hosts: config.show_wsl_host,
            status: "sampling...".to_string(),
            cpu_scale,
            ..Self::default()
        }
    }

    fn key(&mut self, code: KeyCode) -> bool {
        if code == KeyCode::Char('?') {
            self.help = !self.help;
            self.help_scroll = 0;
            return false;
        }
        if self.help {
            match code {
                KeyCode::Esc => self.help = false,
                KeyCode::Char('q') => return true,
                KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                _ => {}
            }
            return false;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('t') => {
                self.tree = !self.tree;
                self.scroll = 0;
            }
            KeyCode::Char('i') => self.hide_infra = !self.hide_infra,
            KeyCode::Char('h') => self.show_hosts = !self.show_hosts,
            KeyCode::Char('0') => self.hide_zero = !self.hide_zero,
            KeyCode::Char('c' | 'm' | 'n') => {
                self.query.sort.key = match code {
                    KeyCode::Char('m') => SortKey::Memory,
                    KeyCode::Char('n') => SortKey::Name,
                    _ => SortKey::Cpu,
                };
                self.scroll = 0;
            }
            KeyCode::Char('r') => {
                self.query.sort.order = self.query.sort.order.reverse();
                self.scroll = 0;
            }
            _ => {}
        }
        false
    }

    fn clamp_scroll(&mut self, height: usize) {
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(height));
    }

    fn apply_sample(&mut self, result: Result<MonitorSnapshot, String>) {
        match result {
            Ok(snapshot) => {
                self.status = if snapshot.warnings.is_empty() {
                    String::new()
                } else {
                    snapshot.warnings.join("; ")
                };
                self.snapshot = Some(snapshot);
                self.rebuild_lines();
            }
            Err(error) => {
                self.status = error;
                if let Some(snapshot) = &mut self.snapshot {
                    snapshot.host_cpu_percent = None;
                    snapshot.host_memory = None;
                    let now = std::time::Instant::now();
                    snapshot.host_history.cpu.record(now, None);
                    snapshot.host_history.memory.record(now, None);
                    snapshot.environment_summary = Default::default();
                }
            }
        }
    }

    fn rebuild_lines(&mut self) {
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        self.query.hide_infra = self.hide_infra;
        self.query.show_wsl_host = self.show_hosts;
        snapshot.requery(&self.query);
        let output = if self.tree {
            render::tree(snapshot, self.cpu_scale)
        } else {
            render::flat(snapshot, self.cpu_scale)
        };
        self.lines = output
            .lines()
            .skip(if self.tree { 2 } else { 1 })
            .enumerate()
            .filter(|(_, line)| !self.hide_zero || !line.contains(" 0.00%"))
            .map(|(index, line)| {
                if !self.tree && index == 1 && !line.is_empty() && line.bytes().all(|ch| ch == b'-')
                {
                    Line::styled(line.to_owned(), separator_style(self.colors))
                } else {
                    header::resource_line(line, self.colors)
                }
            })
            .collect();
    }
}

struct SamplingWorker {
    receiver: mpsc::Receiver<Result<MonitorSnapshot, String>>,
    config: Arc<Mutex<MonitorConfig>>,
    stop: Arc<AtomicBool>,
    details: Arc<AtomicBool>,
}

impl SamplingWorker {
    fn start(config: MonitorConfig, distro: Option<String>, initial_tree: bool) -> Self {
        let explicit_details = config.show_container_processes;
        let shared_config = Arc::new(Mutex::new(config));
        let stop = Arc::new(AtomicBool::new(false));
        let details = Arc::new(AtomicBool::new(initial_tree || explicit_details));
        let (sender, receiver) = mpsc::channel();
        let worker_config = Arc::clone(&shared_config);
        let worker_stop = Arc::clone(&stop);
        let worker_details = Arc::clone(&details);
        thread::spawn(move || {
            stream::run(worker_config, distro, worker_details, worker_stop, sender)
        });
        Self {
            receiver,
            config: shared_config,
            stop,
            details,
        }
    }

    fn set_details(&self, tree: bool) {
        let explicit = self
            .config
            .lock()
            .map(|config| config.show_container_processes)
            .unwrap_or(false);
        self.details.store(tree || explicit, Ordering::Relaxed);
    }
}

impl Drop for SamplingWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}
impl TerminalGuard {
    fn new() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout()))?,
        })
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    use super::{draw_ui, layout_areas, HeaderMode, Line, Rect};
    use crate::monitor::MonitorConfig;
    use crate::render::CpuScale;
    use crossterm::event::KeyCode;
    use std::time::Duration;

    fn layout_state() -> State {
        use crate::model::{ContainerProcessUsage, EnvironmentKind, HostMemory, ResourceKind};
        let config = MonitorConfig {
            sort: Default::default(),
            interval: Duration::from_secs(3),
            limit: 20,
            show_wsl_host: false,
            wsl_only: false,
            no_wslc: false,
            no_docker: false,
            hide_infra: false,
            show_container_processes: true,
            container_process_limit: 5,
            collect_windows_applications: false,
        };
        let mut container = crate::query::tests::row("container", 5.0, 1024);
        container.environment = EnvironmentKind::Docker;
        container.kind = ResourceKind::Container;
        let mut child = crate::query::tests::row("child-process", 2.0, 512);
        child.environment = EnvironmentKind::Docker;
        child.source = Some(container.id.clone());
        let tree = crate::attribution::build_tree_with_docker(
            16,
            &[],
            &[],
            &[],
            &[ContainerProcessUsage {
                resource: container.clone(),
                processes: vec![child.clone()],
                host_pids: vec![],
            }],
        );
        let mut snapshot = crate::monitor::MonitorSnapshot::from_collected(
            vec![container, child],
            tree,
            vec![],
            &config,
        );
        snapshot.host_cpu_percent = Some(25.0);
        snapshot.host_memory = Some(HostMemory {
            total_bytes: 32 * 1073741824,
            available_bytes: 16 * 1073741824,
        });
        let mut state = State::from_config(&config, false, CpuScale::Core);
        state.colors = true;
        state.apply_sample(Ok(snapshot));
        state
    }

    #[test]
    fn three_layers_have_no_extra_title_and_keep_table_columns_and_grouping() {
        use ratatui::{backend::TestBackend, Terminal};
        for width in [40, 79, 80, 119, 120, 160] {
            let mut state = layout_state();
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|frame| {
                    draw_ui(
                        frame,
                        &mut state,
                        HeaderMode::Compact,
                        false,
                        Duration::from_secs(3),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = (0..12)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            assert!(rows[0].starts_with("CPU "));
            assert!(rows[1].starts_with("RAM "));
            assert!(rows[2].trim_end().chars().all(|ch| ch == '─' || ch == '-'));
            assert_eq!(
                rows[2].trim_end().chars().count(),
                usize::from(width.min(97))
            );
            assert!(rows[3].starts_with("ENV "));
            assert!(rows[4].starts_with("---"));
            assert_eq!(buffer[(0, 2)].modifier, buffer[(0, 4)].modifier);
            assert!(buffer[(0, 4)]
                .modifier
                .contains(ratatui::style::Modifier::DIM));
            for y in [3, 5] {
                assert!(!buffer[(0, y)]
                    .modifier
                    .contains(ratatui::style::Modifier::DIM));
            }
            assert!(rows[11].contains("flat cpu↓"));
            assert!(rows[11].contains("q quit"));
            assert!(!rows.iter().any(|row| row.contains("Resources")
                || row.contains("Host logical CPUs")
                || row.contains("updated")));
            assert_eq!(buffer[(0, 5)].fg, ratatui::style::Color::Cyan);
            let expected: Vec<_> =
                crate::render::flat(state.snapshot.as_ref().unwrap(), state.cpu_scale)
                    .lines()
                    .skip(1)
                    .map(str::to_owned)
                    .collect();
            assert_eq!(
                state
                    .lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(expected.iter().any(|row| row.contains("child-process")));
            state.key(KeyCode::Char('t'));
            state.rebuild_lines();
            let expected: Vec<_> =
                crate::render::tree(state.snapshot.as_ref().unwrap(), state.cpu_scale)
                    .lines()
                    .skip(2)
                    .map(str::to_owned)
                    .collect();
            assert_eq!(
                state
                    .lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn footer_preserves_view_sort_and_quit_across_widths_and_options() {
        use crate::query::{SortKey, SortOrder};
        let mut state = layout_state();
        for (key, order, label) in [
            (SortKey::Cpu, SortOrder::Desc, "cpu↓"),
            (SortKey::Cpu, SortOrder::Asc, "cpu↑"),
            (SortKey::Memory, SortOrder::Desc, "mem↓"),
            (SortKey::Name, SortOrder::Asc, "name↑"),
        ] {
            state.query.sort.key = key;
            state.query.sort.order = order;
            for tree in [true, false] {
                state.tree = tree;
                for width in [20, 40, 79, 80, 119, 120] {
                    let text = state.footer(width, Duration::from_secs(3));
                    assert!(Line::raw(text.as_str()).width() <= usize::from(width));
                    assert!(text.contains(if tree { "tree" } else { "flat" }));
                    assert!(text.contains(label));
                    assert!(text.contains("q quit"));
                    assert!(!text.contains("updated"));
                    if width >= 80 {
                        assert!(text.contains("3.0s"));
                        assert!(text.starts_with('['));
                        assert!(text.contains("3.0s]  q quit"));
                    }
                    if width >= 120 {
                        for hint in ["? help", "t tree", "i infra:on", "h hosts:off", "0 zero:on"] {
                            assert!(text.contains(hint));
                        }
                    }
                }
            }
        }
        for key in ['i', 'h', '0'] {
            state.key(KeyCode::Char(key));
        }
        let text = state.footer(120, Duration::from_secs(3));
        for label in ["i infra:off", "h hosts:on", "0 zero:off"] {
            assert!(text.contains(label));
        }
        state.status = "収集エラー: Windows unavailable\n詳細".repeat(10);
        for width in [0, 6, 16, 20, 40, 80, 120] {
            let text = state.footer(width, Duration::from_secs(3));
            assert!(Line::raw(text.as_str()).width() <= usize::from(width));
            assert!(!text.contains('\n'));
            if width >= 6 {
                assert!(text.contains("q quit"));
            }
        }
        assert!(state
            .help_text(Duration::from_secs(3))
            .contains(&state.status));
    }

    #[test]
    fn footer_keeps_scale_inside_status_and_drops_interval_before_scale() {
        let mut state = layout_state();
        for (scale, label) in [(CpuScale::Core, "core"), (CpuScale::Host, "host")] {
            state.cpu_scale = scale;
            for width in [40, 60, 79, 80, 119, 120, 160] {
                let text = state.footer(width, Duration::from_secs(3));
                assert!(text.starts_with(&format!("[flat cpu↓ {label} 3.0s]  q quit")));
                assert!(!text.contains("cpu:"));
                assert!(Line::raw(text).width() <= usize::from(width));
            }
            let text = state.footer(24, Duration::from_secs(3));
            assert_eq!(text, format!("[flat cpu↓ {label}]  q quit"));
        }
    }

    #[test]
    fn layout_reserves_two_summary_rows_and_one_footer_without_a_table_border() {
        for height in [4, 6, 12, 24] {
            let [summary, separator, table, footer] =
                layout_areas(Rect::new(0, 0, 80, height), HeaderMode::Compact);
            assert_eq!(summary.height, 2);
            assert_eq!(separator.height, u16::from(height >= 5));
            assert_eq!(table.y, 2 + separator.height);
            assert_eq!(table.height, height - 3 - separator.height);
            assert_eq!(footer.y, height - 1);
            assert_eq!(footer.height, 1);
        }
        let [summary, separator, table, footer] =
            layout_areas(Rect::new(0, 0, 80, 12), HeaderMode::Classic);
        assert_eq!(summary.height, 1);
        assert_eq!(separator.height, 1);
        assert_eq!(table.height, 9);
        assert_eq!(footer.height, 1);
    }
    #[test]
    fn summary_separator_is_neutral_and_supports_ascii_and_no_color() {
        for width in [0, 1, 60, 80, 120] {
            for ascii in [true, false] {
                for colors in [true, false] {
                    let line = super::summary_separator(width, colors, ascii);
                    assert_eq!(line.width(), usize::from(width));
                    assert_eq!(
                        line.to_string(),
                        if ascii { "-" } else { "─" }.repeat(usize::from(width))
                    );
                    assert_eq!(line.style.fg, None);
                    assert_eq!(line.style.bg, None);
                    assert_eq!(
                        line.style
                            .add_modifier
                            .contains(ratatui::style::Modifier::DIM),
                        colors
                    );
                }
            }
        }
    }
    #[test]
    fn release_display_options_do_not_change_flat_or_tree_json() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = layout_state();
        let json = |state: &State| {
            let snapshot = state.snapshot.as_ref().unwrap();
            (
                serde_json::to_value(&snapshot.pid_resources).unwrap(),
                serde_json::to_value(&snapshot.tree).unwrap(),
            )
        };
        let expected = json(&state);
        for mode in [HeaderMode::Classic, HeaderMode::Compact] {
            for colors in [false, true] {
                state.colors = colors;
                for width in [75, 90, 120, 200] {
                    let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
                    terminal
                        .draw(|frame| {
                            draw_ui(frame, &mut state, mode, false, Duration::from_secs(3))
                        })
                        .unwrap();
                    assert_eq!(json(&state), expected);
                }
            }
        }
    }

    #[test]
    fn help_scroll_reaches_last_wrapped_row_and_reclamps_after_resize() {
        use ratatui::{
            backend::TestBackend,
            buffer::Buffer,
            widgets::{Paragraph, Widget, Wrap},
            Terminal,
        };
        let mut state = layout_state();
        state.help = true;
        state.status = "A long collector status with words that wrap at boundaries. 日本語の状態も表示します。".repeat(5);
        let interval = Duration::from_secs(3);
        for width in [19, 40, 80, 120, 19] {
            // Independently render the complete help into a tall buffer and find
            // its actual last occupied row, rather than duplicating the counter.
            let text = state.help_text(interval);
            let mut full = Buffer::empty(Rect::new(0, 0, width, 2000));
            Paragraph::new(text.clone())
                .wrap(Wrap { trim: false })
                .render(full.area, &mut full);
            let last = (0..2000)
                .rev()
                .find(|&y| (0..width).any(|x| full[(x, y)].symbol() != " "))
                .unwrap();
            let [_, _, table, _] = layout_areas(Rect::new(0, 0, width, 12), HeaderMode::Compact);
            let expected_scroll = (last + 1).saturating_sub(table.height);
            if width == 19 {
                let old_count: usize = text
                    .lines()
                    .map(|line| Line::raw(line).width().div_ceil(usize::from(width)).max(1))
                    .sum();
                assert!(usize::from(last + 1) > old_count);
            }
            for _ in 0..200 {
                state.key(KeyCode::PageDown);
            }
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|frame| draw_ui(frame, &mut state, HeaderMode::Compact, false, interval))
                .unwrap();
            assert_eq!(state.help_scroll, expected_scroll);
            let buffer = terminal.backend().buffer();
            for x in 0..width {
                assert_eq!(
                    buffer[(x, table.y + table.height - 1)].symbol(),
                    full[(x, last)].symbol()
                );
            }
            state.key(KeyCode::PageUp);
            assert_eq!(state.help_scroll, expected_scroll.saturating_sub(10));
        }
    }

    #[test]
    fn classic_header_restores_view_cpu_scale_sort_and_interval() {
        let mut state = layout_state();
        state.snapshot.as_mut().unwrap().host_cpu_percent = Some(42.5);
        for tree in [false, true] {
            state.tree = tree;
            let view = if tree { "tree" } else { "flat" };
            assert_eq!(state.classic_header(160, Duration::from_secs(3)),
                format!(" {view} | Host CPU 42.5% | CPU 1 core = 100% | sort cpu desc | interval 3000ms"));
            for width in [0, 40, 80, 160] {
                use ratatui::{backend::TestBackend, Terminal};
                let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
                terminal
                    .draw(|frame| {
                        draw_ui(
                            frame,
                            &mut state,
                            HeaderMode::Classic,
                            false,
                            Duration::from_secs(3),
                        )
                    })
                    .unwrap();
                let row: String = (0..width)
                    .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
                    .collect();
                assert_eq!(
                    row.trim_end(),
                    state
                        .classic_header(width, Duration::from_secs(3))
                        .trim_end()
                );
            }
        }
    }

    #[test]
    fn tree_commands_do_not_stretch_summary_separator() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = layout_state();
        state.tree = true;
        state.rebuild_lines();
        for width in [80, 160, 240] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            let mut lengths = Vec::new();
            for name in ["short".to_owned(), "command".repeat(100)] {
                state.lines = vec![Line::raw("Windows applications"), Line::raw(name)];
                terminal
                    .draw(|frame| {
                        draw_ui(
                            frame,
                            &mut state,
                            HeaderMode::Compact,
                            false,
                            Duration::from_secs(3),
                        )
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                lengths.push(
                    (0..width)
                        .filter(|&x| matches!(buffer[(x, 2)].symbol(), "─" | "-"))
                        .count(),
                );
            }
            assert_eq!(lengths[0], lengths[1]);
            assert!(lengths[0] < 100);
        }
    }

    #[test]
    fn summary_separator_tracks_content_instead_of_terminal_or_commands() {
        let summary = [Line::raw("s".repeat(94))];
        let table = [
            Line::raw("ENV"),
            Line::raw("-".repeat(97)),
            Line::raw("x".repeat(300)),
        ];
        for width in [0, 60, 80, 120, 240] {
            assert_eq!(
                super::summary_separator_width(width, &summary, &table),
                width.min(97)
            );
        }
        assert_eq!(super::summary_separator_width(240, &summary, &[]), 94);
        assert_eq!(
            super::summary_separator_width(240, &[Line::raw("s".repeat(110))], &table),
            110
        );
        assert_eq!(super::summary_separator_width(240, &[], &[]), 0);
    }
    #[test]
    fn updates_navigation_and_toggles() {
        let mut state = State::default();
        state.key(KeyCode::Down);
        state.key(KeyCode::Char('t'));
        state.key(KeyCode::Char('i'));
        assert_eq!(state.scroll, 0);
        assert!(state.tree && state.hide_infra);
        assert!(state.key(KeyCode::Char('q')));
    }

    #[test]
    fn applies_initial_interactive_view_options() {
        let config = MonitorConfig {
            sort: Default::default(),
            interval: Duration::from_millis(500),
            limit: 12,
            show_wsl_host: true,
            wsl_only: false,
            no_wslc: true,
            no_docker: true,
            hide_infra: true,
            show_container_processes: false,
            container_process_limit: 5,
            collect_windows_applications: true,
        };
        let state = State::from_config(&config, true, CpuScale::Core);
        assert!(state.tree);
        assert!(state.hide_infra);
        assert!(state.show_hosts);
        assert_eq!(state.cpu_scale, CpuScale::Core);
    }

    #[test]
    fn sort_keys_requery_untruncated_snapshot_immediately_and_survive_updates() {
        use crate::query::{SortKey, SortOrder};
        let config = MonitorConfig {
            sort: Default::default(),
            interval: Duration::from_secs(30),
            limit: 1,
            show_wsl_host: false,
            wsl_only: true,
            no_wslc: true,
            no_docker: true,
            hide_infra: false,
            show_container_processes: false,
            container_process_limit: 1,
            collect_windows_applications: false,
        };
        let sample = || {
            let rows = vec![
                crate::query::tests::row("busy", 10.0, 1),
                crate::query::tests::row("large", 1.0, 100),
            ];
            let tree = crate::attribution::build_tree_with_docker(16, &[], &rows, &[], &[]);
            let mut snapshot =
                crate::monitor::MonitorSnapshot::from_collected(rows, tree, vec![], &config);
            snapshot.host_cpu_percent = Some(42.5);
            snapshot
        };
        let mut state = State::from_config(&config, false, CpuScale::Core);
        assert_eq!(super::host_cpu_label(None), "N/A");
        state.apply_sample(Ok(sample()));
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "busy");
        state.scroll = 10;
        state.key(KeyCode::Char('m'));
        state.rebuild_lines();
        assert_eq!(state.scroll, 0);
        assert_eq!(state.query.sort.key, SortKey::Memory);
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "large");
        assert_eq!(super::host_cpu_label(state.snapshot.as_ref()), "42.5%");
        state.apply_sample(Ok(sample()));
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "large");
        state.key(KeyCode::Char('r'));
        state.rebuild_lines();
        assert_eq!(state.query.sort.order, SortOrder::Asc);
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "busy");
        state.key(KeyCode::Char('n'));
        state.rebuild_lines();
        assert_eq!(state.query.sort.key, SortKey::Name);
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "busy");
        state.key(KeyCode::Char('c'));
        state.rebuild_lines();
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "large");
        state.apply_sample(Err("offline".into()));
        assert_eq!(super::host_cpu_label(state.snapshot.as_ref()), "N/A");
        assert!(state.snapshot.as_ref().unwrap().host_memory.is_none());
        assert_eq!(
            state.snapshot.as_ref().unwrap().environment_summary.0,
            [None; 4]
        );
    }

    #[test]
    fn help_navigation_does_not_change_resource_view() {
        let mut state = State::default();
        assert!(!state.key(KeyCode::Char('?')));
        assert!(state.help);
        state.key(KeyCode::Down);
        assert_eq!(state.help_scroll, 1);
        assert_eq!(state.scroll, 0);
        state.key(KeyCode::Char('t'));
        assert!(!state.tree);
        assert!(!state.key(KeyCode::Esc));
        assert!(!state.help);
    }
}
#[test]
fn windows_key_release_does_not_undo_toggles() {
    use crossterm::event::{KeyEvent, KeyModifiers};
    let mut state = State::default();
    for code in [KeyCode::Char('r'), KeyCode::Char('t')] {
        for kind in [KeyEventKind::Press, KeyEventKind::Release] {
            let event = Event::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind));
            if let Some(code) = actionable_key(event) {
                state.key(code);
            }
        }
    }
    assert_eq!(state.query.sort.order, crate::query::SortOrder::Asc);
    assert!(state.tree);
    let repeat = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Down,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    state.key(actionable_key(repeat).unwrap());
    assert_eq!(state.scroll, 1);
}
