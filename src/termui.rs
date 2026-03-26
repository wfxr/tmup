use owo_colors::OwoColorize;

/// Visual accent style applied to labeled output lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    /// Plain bold text with no color.
    Bold,
    /// Cyan bold, used for informational labels.
    Info,
    /// Green bold, used for successful-operation labels.
    Success,
    /// Yellow bold, used for warning labels.
    Warning,
    /// Red bold, used for error labels.
    Error,
    /// Dimmed text, used for de-emphasized labels.
    Muted,
}

/// Return `text` wrapped in ANSI bold formatting.
pub fn bold(text: &str) -> String {
    format!("{}", text.bold())
}

/// Format a right-aligned label followed by a message with no ANSI styling.
pub fn format_plain_labeled_line(label: &str, width: usize, message: &str) -> String {
    format!("{label:>width$} {message}")
}

/// Format a right-aligned, ANSI-styled label followed by a plain message.
pub fn format_styled_labeled_line(
    label: &str,
    width: usize,
    message: &str,
    accent: Accent,
) -> String {
    format!("{} {}", style_labeled_text(label, width, accent), message)
}

fn style_labeled_text(label: &str, width: usize, accent: Accent) -> String {
    let padded = format!("{label:>width$}");
    match accent {
        Accent::Bold => format!("{}", padded.bold()),
        Accent::Info => format!("{}", padded.bold().cyan()),
        Accent::Success => format!("{}", padded.bold().green()),
        Accent::Warning => format!("{}", padded.bold().yellow()),
        Accent::Error => format!("{}", padded.bold().red()),
        Accent::Muted => format!("{}", padded.dimmed()),
    }
}
