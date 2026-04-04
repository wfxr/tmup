use owo_colors::OwoColorize;
use unicode_width::UnicodeWidthChar;

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

/// Truncate a single logical line to the requested display width.
pub fn truncate_display_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut current_width = 0;
    let mut needs_truncation = false;
    for ch in text.chars() {
        current_width += UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width > max_width {
            needs_truncation = true;
            break;
        }
    }

    if !needs_truncation {
        return text.to_string();
    }

    let ellipsis = '…';
    let ellipsis_width = UnicodeWidthChar::width(ellipsis).unwrap_or(1);
    if max_width <= ellipsis_width {
        return ellipsis.to_string();
    }

    let mut truncated = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width + ellipsis_width > max_width {
            break;
        }
        truncated.push(ch);
        width += ch_width;
    }
    truncated.push(ellipsis);
    truncated
}

#[cfg(test)]
mod tests {
    use super::truncate_display_width;

    #[test]
    fn truncate_single_line_respects_display_width() {
        assert_eq!(truncate_display_width("abcdef", 4), "abc…");
        assert_eq!(truncate_display_width("你好世界", 5), "你好…");
        assert_eq!(truncate_display_width("hi", 4), "hi");
    }
}
