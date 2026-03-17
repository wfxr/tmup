use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::planner::PluginStatus;

/// Render detail view for a selected plugin.
pub fn render_detail(frame: &mut Frame, area: Rect, status: &PluginStatus) {
    let text = vec![
        Line::from(vec![
            Span::styled("ID: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&status.id),
        ]),
        Line::from(vec![
            Span::styled("Name: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&status.name),
        ]),
        Line::from(vec![
            Span::styled("Source: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&status.source),
        ]),
        Line::from(vec![
            Span::styled("Kind: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&status.kind),
        ]),
        Line::from(vec![
            Span::styled("State: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(status.state.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Last Result: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(status.last_result.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "Lock Commit: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(status.lock_commit.as_deref().unwrap_or("-")),
        ]),
    ];

    let detail = Paragraph::new(text)
        .block(Block::default().title(" Detail ").borders(Borders::ALL))
        .wrap(Wrap { trim: false });

    frame.render_widget(detail, area);
}
