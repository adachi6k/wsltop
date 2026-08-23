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
use std::process::Command;
use std::time::{Duration, Instant};

pub fn run(interval: Duration) -> Result<(), Box<dyn Error>> {
    let mut terminal = TerminalGuard::new()?;
    let mut state = State::default();
    refresh(&mut state, interval);
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
                        " wsltop Phase 5 | {} | refresh {}ms ",
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
                    refresh(&mut state, interval);
                    last_refresh = Instant::now();
                }
            }
        }
        if last_refresh.elapsed() >= interval {
            refresh(&mut state, interval);
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

fn refresh(state: &mut State, interval: Duration) {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            state.status = error.to_string();
            return;
        }
    };
    let mut command = Command::new(executable);
    command.args([
        "--interval-ms",
        &interval.as_millis().to_string(),
        "--limit",
        "200",
    ]);
    if state.tree {
        command.arg("--tree");
    }
    if state.hide_infra {
        command.arg("--hide-infra");
    }
    if state.show_hosts {
        command.arg("--show-wsl-host");
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            state.lines = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !state.hide_zero || !line.contains(" 0.00%"))
                .map(|line| Line::raw(line.to_string()))
                .collect();
            state.status = "updated".to_string();
        }
        Ok(output) => state.status = String::from_utf8_lossy(&output.stderr).trim().to_string(),
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
