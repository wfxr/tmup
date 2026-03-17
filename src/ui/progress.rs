use ratatui::{prelude::*, widgets::Paragraph};

/// Render a simple progress indicator for ongoing operations.
pub fn render_progress(frame: &mut Frame, area: Rect, message: &str) {
    let p = Paragraph::new(format!(" {message}")).style(Style::default().fg(Color::Yellow));
    frame.render_widget(p, area);
}
