use crate::monitor::{Monitor, MonitorConfig, MonitorSnapshot};
use crate::render;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use std::error::Error;
use std::io::{self, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

pub fn run(config: MonitorConfig, initial_tree: bool) -> Result<(), Box<dyn Error>> {
    let interval = config.interval;
    let mut terminal = TerminalGuard::new()?;
    let mut state = State::from_config(&config, initial_tree);
    let worker = SamplingWorker::start(config);

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
                Paragraph::new(Line::styled(
                    format!(
                        " wsltop {} | {} | refresh {}ms ",
                        env!("CARGO_PKG_VERSION"),
                        if state.tree { "tree" } else { "flat" },
                        interval.as_millis()
                    ),
                    Style::default().fg(Color::Black).bg(Color::Cyan),
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
                if matches!(key.code, KeyCode::Char('t' | 'i' | 'h' | '0')) {
                    state.rebuild_lines();
                    worker.update_filters(state.hide_infra, state.show_hosts);
                }
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct State {
    lines: Vec<Line<'static>>,
    scroll: usize,
    tree: bool,
    hide_infra: bool,
    show_hosts: bool,
    status: String,
    hide_zero: bool,
    snapshot: Option<MonitorSnapshot>,
}

impl State {
    fn from_config(config: &MonitorConfig, tree: bool) -> Self {
        Self {
            tree,
            hide_infra: config.hide_infra,
            show_hosts: config.show_wsl_host,
            status: "sampling...".to_string(),
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
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let output = if self.tree {
            render::tree(snapshot)
        } else {
            render::flat(snapshot)
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
}

impl SamplingWorker {
    fn start(config: MonitorConfig) -> Self {
        let shared_config = Arc::new(Mutex::new(config));
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let worker_config = Arc::clone(&shared_config);
        let worker_stop = Arc::clone(&stop);
        thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let config = match worker_config.lock() {
                    Ok(config) => config.clone(),
                    Err(_) => break,
                };
                let retry_interval = config.interval;
                let result = Monitor::new(config)
                    .sample()
                    .map_err(|error| error.to_string());
                let failed = result.is_err();
                if sender.send(result).is_err() {
                    break;
                }
                if failed {
                    thread::sleep(retry_interval);
                }
            }
        });
        Self {
            receiver,
            config: shared_config,
            stop,
        }
    }

    fn update_filters(&self, hide_infra: bool, show_wsl_host: bool) {
        if let Ok(mut config) = self.config.lock() {
            config.hide_infra = hide_infra;
            config.show_wsl_host = show_wsl_host;
        }
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
            interval: Duration::from_millis(500),
            limit: 12,
            show_wsl_host: true,
            wsl_only: false,
            no_wslc: true,
            no_docker: true,
            hide_infra: true,
        };
        let state = State::from_config(&config, true);
        assert!(state.tree);
        assert!(state.hide_infra);
        assert!(state.show_hosts);
    }
}
