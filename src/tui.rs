use crate::monitor::{MonitorConfig, MonitorSnapshot};
use crate::query::{ResourceQuery, SortKey};
use crate::render;
use crate::render::CpuScale;
use crate::stream;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
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
) -> Result<(), Box<dyn Error>> {
    let interval = config.interval;
    let mut terminal = TerminalGuard::new()?;
    let mut state = State::from_config(&config, initial_tree, cpu_scale);
    let worker = SamplingWorker::start(config, distro, initial_tree);

    loop {
        for result in worker.receiver.try_iter() {
            state.apply_sample(result);
        }
        terminal.terminal.draw(|frame| {
            let [header, body, footer] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            frame.render_widget(
                Paragraph::new(format!(
                    " {} | CPU {} | sort {} {} | interval {}ms",
                    if state.tree { "tree" } else { "flat" },
                    state.cpu_scale.label(),
                    state.query.sort.key.label(),
                    state.query.sort.order.label(),
                    interval.as_millis()
                )),
                header,
            );
            let height = body.height.saturating_sub(2) as usize;
            state.clamp_scroll(height);
            let visible = state.lines.iter().skip(state.scroll).take(height).cloned();
            frame.render_widget(
                Paragraph::new(visible.collect::<Vec<_>>())
                    .block(Block::default().borders(Borders::ALL).title("Resources")),
                body,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    " q/Esc quit  ↑↓/Pg scroll  t tree  i infra:{}  h hosts:{}  0 zero:{}  {}",
                    on_off(!state.hide_infra),
                    on_off(state.show_hosts),
                    on_off(!state.hide_zero),
                    state.status
                )),
                footer,
            );
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if state.key(key.code) {
                    break;
                }
                if matches!(
                    key.code,
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

#[derive(Default)]
struct State {
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
                    "updated".to_string()
                } else {
                    snapshot.warnings.join("; ")
                };
                self.snapshot = Some(snapshot);
                self.rebuild_lines();
            }
            Err(error) => self.status = error,
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
            .filter(|line| !self.hide_zero || !line.contains(" 0.00%"))
            .map(|line| Line::raw(line.to_string()))
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
    use crate::monitor::MonitorConfig;
    use crate::render::CpuScale;
    use crossterm::event::KeyCode;
    use std::time::Duration;
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
            crate::monitor::MonitorSnapshot::from_collected(rows, tree, vec![], &config)
        };
        let mut state = State::from_config(&config, false, CpuScale::Core);
        state.apply_sample(Ok(sample()));
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "busy");
        state.scroll = 10;
        state.key(KeyCode::Char('m'));
        state.rebuild_lines();
        assert_eq!(state.scroll, 0);
        assert_eq!(state.query.sort.key, SortKey::Memory);
        assert_eq!(state.snapshot.as_ref().unwrap().resources[0].name, "large");
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
    }
}
