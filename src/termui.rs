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

/// Format a styled labeled line and clamp the message to `max_width`.
pub fn format_styled_labeled_line_clamped(
    label: &str,
    width: usize,
    message: &str,
    accent: Accent,
    max_width: usize,
) -> String {
    let label_segment = format!("{label:>width$}");
    let label_width =
        label_segment.chars().map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0)).sum::<usize>();
    let available_message_width = max_width.saturating_sub(label_width.saturating_add(1));
    let clamped = truncate_display_width(message, available_message_width);
    format_styled_labeled_line(label, width, &clamped, accent)
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
    use super::{Accent, format_styled_labeled_line_clamped, truncate_display_width};

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut in_escape = false;
        let mut in_csi = false;
        for ch in s.chars() {
            if in_csi {
                if ch.is_ascii() && (ch as u8) >= 0x40 && (ch as u8) <= 0x7e {
                    in_csi = false;
                }
                continue;
            }
            if in_escape {
                in_escape = false;
                if ch == '[' {
                    in_csi = true;
                }
                continue;
            }
            if ch == '\x1b' {
                in_escape = true;
                continue;
            }
            out.push(ch);
        }
        out
    }

    #[test]
    fn truncate_single_line_respects_display_width() {
        assert_eq!(truncate_display_width("abcdef", 4), "abc…");
        assert_eq!(truncate_display_width("你好世界", 5), "你好…");
        assert_eq!(truncate_display_width("hi", 4), "hi");
    }

    #[test]
    fn format_styled_labeled_line_clamps_message_width() {
        let line = format_styled_labeled_line_clamped(
            "Label",
            8,
            "abcdefghijklmnopqrstuvwxyz",
            Accent::Info,
            16,
        );
        let plain = strip_ansi(&line);
        assert!(plain.chars().count() <= 16);
    }
}
