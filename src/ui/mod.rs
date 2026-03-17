pub mod detail;
pub mod plugin_list;
pub mod progress;

use anyhow::Result;
use crossterm::{
    ExecutableCommand,
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::io;

use crate::planner::{PluginState, PluginStatus};

/// TUI application state.
pub struct App {
    pub rows:        Vec<PluginStatus>,
    pub busy:        bool,
    pub selected:    usize,
    pub search:      Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(rows: Vec<PluginStatus>, busy: bool) -> Self {
        Self { rows, busy, selected: 0, search: None, should_quit: false }
    }

    pub fn filtered_rows(&self) -> Vec<&PluginStatus> {
        match &self.search {
            None => self.rows.iter().collect(),
            Some(query) => self
                .rows
                .iter()
                .filter(|r| r.id.contains(query.as_str()) || r.name.contains(query.as_str()))
                .collect(),
        }
    }

    fn summary(&self) -> (usize, usize, usize, usize) {
        let installed = self
            .rows
            .iter()
            .filter(|r| r.state == PluginState::Installed)
            .count();
        let updates = self
            .rows
            .iter()
            .filter(|r| r.state == PluginState::Outdated)
            .count();
        let missing = self
            .rows
            .iter()
            .filter(|r| r.state == PluginState::Missing)
            .count();
        let pinned = self
            .rows
            .iter()
            .filter(|r| matches!(r.state, PluginState::PinnedTag | PluginState::PinnedCommit))
            .count();
        (installed, updates, missing, pinned)
    }
}

/// Render the app to a frame.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header + summary
            Constraint::Min(5),    // plugin list
            Constraint::Length(2), // footer keybindings
        ])
        .split(area);

    render_header(app, frame, chunks[0]);
    plugin_list::render(app, frame, chunks[1]);
    render_footer(app, frame, chunks[2]);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let (installed, updates, missing, pinned) = app.summary();

    let header_text = if app.busy {
        format!(
            " lazy.tmux  |  Installed {}  Updates {}  Missing {}  Pinned {}  |  ⏳ BUSY",
            installed, updates, missing, pinned
        )
    } else {
        format!(
            " lazy.tmux  |  Installed {}  Updates {}  Missing {}  Pinned {}",
            installed, updates, missing, pinned
        )
    };

    let header = Paragraph::new(header_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::BOTTOM));

    frame.render_widget(header, area);
}

fn render_footer(_app: &App, frame: &mut Frame, area: Rect) {
    let footer = Paragraph::new(
        " I install  U update  C clean  R restore  / search  l log  d diff  x remove  ? help  q quit",
    )
    .style(Style::default().fg(Color::DarkGray));

    frame.render_widget(footer, area);
}

/// Run the TUI event loop.
pub fn run_tui(mut app: App) -> Result<()> {
    terminal::enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|frame| render(&app, frame))?;

        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Char('j') | KeyCode::Down => {
                    let max = app.filtered_rows().len().saturating_sub(1);
                    app.selected = (app.selected + 1).min(max);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.selected = app.selected.saturating_sub(1);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    terminal::disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
