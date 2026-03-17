use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Row, Table},
};

use crate::{
    planner::{LastResult, PluginState},
    ui::App,
};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let filtered = app.filtered_rows();

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, status)| {
            let state_style = state_color(status.state);
            let result_str = match status.last_result {
                LastResult::Ok => "",
                LastResult::BuildFailed => " ⚠ build-failed",
                LastResult::None => "",
            };

            let icon = match status.state {
                PluginState::Installed => "✓",
                PluginState::Missing => "!",
                PluginState::Outdated => "↻",
                PluginState::PinnedTag | PluginState::PinnedCommit => "•",
                PluginState::Unmanaged => "?",
                PluginState::Local => "•",
            };

            let commit_short = status
                .lock_commit
                .as_deref()
                .map(|c| &c[..7.min(c.len())])
                .unwrap_or("-");

            let row_style = if i == app.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(format!("  {icon}")),
                Cell::from(status.name.clone()),
                Cell::from(status.kind.clone()),
                Cell::from(format!("{}{result_str}", status.state)).style(state_style),
                Cell::from(commit_short.to_string()),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["", "Name", "Kind", "State", "Commit"]).style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            ),
        )
        .block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, area);
}

fn state_color(state: PluginState) -> Style {
    match state {
        PluginState::Installed => Style::default().fg(Color::Green),
        PluginState::Missing => Style::default().fg(Color::Red),
        PluginState::Outdated => Style::default().fg(Color::Yellow),
        PluginState::PinnedTag | PluginState::PinnedCommit => Style::default().fg(Color::Blue),
        PluginState::Unmanaged => Style::default().fg(Color::Magenta),
        PluginState::Local => Style::default().fg(Color::Cyan),
    }
}
