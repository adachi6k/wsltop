use crate::monitor::{Monitor, MonitorConfig};
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
use std::time::{Duration, Instant};

pub fn run(config: MonitorConfig, initial_tree: bool) -> Result<(), Box<dyn Error>> {
    let interval = config.interval;
    let mut terminal = TerminalGuard::new()?;
    let mut state = State {
        tree: initial_tree,
        hide_infra: config.hide_infra,
        show_hosts: config.show_wsl_host,
        ..State::default()
    };
    let mut monitor = Monitor::new(config);
    refresh(&mut state, &mut monitor);
    let mut last_refresh = Instant::now();

    loop {
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
                    refresh(&mut state, &mut monitor);
                    last_refresh = Instant::now();
                }
            }
        }
        if last_refresh.elapsed() >= interval {
            refresh(&mut state, &mut monitor);
            last_refresh = Instant::now();
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
}

impl State {
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
}

fn refresh(state: &mut State, monitor: &mut Monitor) {
    monitor.config_mut().hide_infra = state.hide_infra;
    monitor.config_mut().show_wsl_host = state.show_hosts;
    match monitor.sample() {
        Ok(snapshot) => {
            let output = if state.tree {
                render::tree(&snapshot)
            } else {
                render::flat(&snapshot)
            };
            state.lines = output
                .lines()
                .filter(|line| !state.hide_zero || !line.contains(" 0.00%"))
                .map(|line| Line::raw(line.to_string()))
                .collect();
            state.status = if snapshot.warnings.is_empty() {
                "updated".to_string()
            } else {
                snapshot.warnings.join("; ")
            };
        }
        Err(error) => state.status = error.to_string(),
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
    use crossterm::event::KeyCode;
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
}
