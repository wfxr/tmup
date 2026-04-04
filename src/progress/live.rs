use std::io::Write;
use std::path::Path;

use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{QueueableCommand, cursor};

use crate::progress::ACTION_WIDTH;
use crate::progress::reducer::{self, ProgressSnapshot};
use crate::progress::render::DisplayLine;
use crate::termui::{self, Accent};

const DEFAULT_TERMINAL_WIDTH: usize = 120;

/// Live fixed-row renderer used by TTY reporter sinks.
pub(crate) struct LiveRenderer<W: Write> {
    writer: W,
    frame_lines: Vec<String>,
    terminal_width: usize,
    initialized: bool,
    frozen: bool,
}

impl<W: Write> LiveRenderer<W> {
    /// Create a live renderer using terminal width when available.
    pub(crate) fn new(writer: W) -> Self {
        let terminal_width =
            terminal::size().map(|(width, _)| width as usize).unwrap_or(DEFAULT_TERMINAL_WIDTH);
        Self::new_with_width(writer, terminal_width)
    }

    fn new_with_width(writer: W, terminal_width: usize) -> Self {
        Self {
            writer,
            frame_lines: Vec::new(),
            terminal_width: terminal_width.max(1),
            initialized: false,
            frozen: false,
        }
    }

    /// Reserve one operation row plus one row for each plugin slot.
    pub(crate) fn bootstrap(&mut self, snapshot: &ProgressSnapshot) {
        if self.frozen {
            return;
        }
        if !self.initialized {
            self.frame_lines = self.placeholder_frame(snapshot);
            let _ = self.writer.queue(cursor::Hide);
            for line in &self.frame_lines {
                let _ = writeln!(self.writer, "{line}");
            }
            let _ = self.writer.flush();
            self.initialized = true;
            return;
        }

        let required_rows = 1 + snapshot.plugins.len();
        while self.frame_lines.len() < required_rows {
            let slot = self.frame_lines.len() - 1;
            let label = snapshot
                .plugins
                .iter()
                .find(|plugin| plugin.slot == slot)
                .map(|plugin| plugin.label.as_str())
                .unwrap_or("plugin");
            let line = self.placeholder_plugin_line(label);
            self.frame_lines.push(line.clone());
            let _ = writeln!(self.writer, "{line}");
        }
        let _ = self.writer.flush();
    }

    /// Update the live frame for one reducer event.
    pub(crate) fn write_reducer_lines(
        &mut self,
        snapshot: &ProgressSnapshot,
        event: &reducer::ProgressEvent,
        lines: Vec<DisplayLine>,
    ) {
        if self.frozen {
            return;
        }
        self.bootstrap(snapshot);

        let Some(row) = row_for_event(snapshot, event) else {
            return;
        };
        let Some(line) = lines.into_iter().next() else {
            return;
        };
        let rendered = termui::format_styled_labeled_line_clamped(
            &line.label,
            ACTION_WIDTH,
            &line.message,
            line.accent,
            self.terminal_width,
        );
        self.write_row(row, rendered);
    }

    /// Render an operation-level failure into the operation row.
    pub(crate) fn write_operation_failure(&mut self, snapshot: &ProgressSnapshot, summary: &str) {
        if self.frozen {
            return;
        }
        self.bootstrap(snapshot);
        let rendered = termui::format_styled_labeled_line_clamped(
            "Failed",
            ACTION_WIDTH,
            &format!("operation {summary}"),
            Accent::Error,
            self.terminal_width,
        );
        self.write_row(0, rendered);
    }

    /// Freeze the final frame and restore cursor state.
    pub(crate) fn finish(
        &mut self,
        snapshot: &ProgressSnapshot,
        command: Option<&'static str>,
        details_path: Option<&Path>,
    ) {
        if self.frozen {
            return;
        }
        self.bootstrap(snapshot);

        if matches!(command, Some("init")) {
            let rendered = termui::format_styled_labeled_line_clamped(
                "Finished",
                ACTION_WIDTH,
                "tmup init",
                Accent::Success,
                self.terminal_width,
            );
            self.write_row(0, rendered);
        }
        if let Some(path) = details_path {
            let details_line = termui::format_styled_labeled_line_clamped(
                "Details",
                ACTION_WIDTH,
                &path.display().to_string(),
                Accent::Muted,
                self.terminal_width,
            );
            let _ = writeln!(self.writer, "{details_line}");
        }

        let _ = self.writer.queue(cursor::Show);
        let _ = self.writer.flush();
        self.frozen = true;
    }

    fn placeholder_frame(&self, snapshot: &ProgressSnapshot) -> Vec<String> {
        let mut rows = vec![self.placeholder_operation_line(); 1 + snapshot.plugins.len()];
        for plugin in &snapshot.plugins {
            let row = plugin.slot + 1;
            if row >= rows.len() {
                rows.resize(row + 1, self.placeholder_operation_line());
            }
            rows[row] = self.placeholder_plugin_line(&plugin.label);
        }
        rows
    }

    fn placeholder_operation_line(&self) -> String {
        termui::format_styled_labeled_line_clamped(
            "Status",
            ACTION_WIDTH,
            "pending",
            Accent::Muted,
            self.terminal_width,
        )
    }

    fn placeholder_plugin_line(&self, label: &str) -> String {
        termui::format_styled_labeled_line_clamped(
            "Pending",
            ACTION_WIDTH,
            label,
            Accent::Muted,
            self.terminal_width,
        )
    }

    fn write_row(&mut self, row: usize, rendered: String) {
        if row >= self.frame_lines.len() {
            return;
        }
        self.frame_lines[row] = rendered.clone();
        if !self.initialized {
            return;
        }

        let up = self.frame_lines.len().saturating_sub(row.saturating_add(1)) as u16;
        if up > 0 {
            let _ = self.writer.queue(cursor::MoveUp(up));
        }
        let _ = self.writer.queue(cursor::MoveToColumn(0));
        let _ = self.writer.queue(Clear(ClearType::CurrentLine));
        let _ = self.writer.queue(Print(rendered));
        if up > 0 {
            let _ = self.writer.queue(cursor::MoveDown(up));
        }
        let _ = self.writer.queue(cursor::MoveToColumn(0));
        let _ = self.writer.flush();
    }
}

fn row_for_event(snapshot: &ProgressSnapshot, event: &reducer::ProgressEvent) -> Option<usize> {
    match event {
        reducer::ProgressEvent::OperationStageChanged { .. } => Some(0),
        reducer::ProgressEvent::PluginStageChanged { id, .. }
        | reducer::ProgressEvent::PluginFinished { id, .. }
        | reducer::ProgressEvent::PluginFailed { id, .. } => {
            snapshot.plugin(id).map(|plugin| plugin.slot + 1)
        }
    }
}

#[cfg(test)]
impl LiveRenderer<Vec<u8>> {
    pub(crate) fn new_for_tests(writer: Vec<u8>, terminal_width: usize) -> Self {
        Self::new_with_width(writer, terminal_width)
    }

    fn frame_len_for_tests(&self) -> usize {
        self.frame_lines.len()
    }

    fn frame_line_for_tests(&self, row: usize) -> Option<&str> {
        self.frame_lines.get(row).map(String::as_str)
    }

    fn frozen_for_tests(&self) -> bool {
        self.frozen
    }

    fn output_for_tests(&self) -> String {
        String::from_utf8_lossy(&self.writer).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::LiveRenderer;
    use crate::progress::model::{OperationStage, PluginOutcome, PluginStage, PluginStageDetail};
    use crate::progress::reducer::{ProgressEvent, ProgressSnapshot, apply_event};
    use crate::progress::render::TranscriptRenderer;

    #[test]
    fn live_renderer_preserves_fixed_plugin_slots() {
        let mut snapshot = ProgressSnapshot::from_ordered_plugins(vec![
            ("github.com/acme/a".to_string(), "plugin-a".to_string()),
            ("github.com/acme/b".to_string(), "plugin-b".to_string()),
        ]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);
        let transcript = TranscriptRenderer::new();

        renderer.bootstrap(&snapshot);
        assert_eq!(renderer.frame_len_for_tests(), 3);

        let plugin_a_row = snapshot.plugin("github.com/acme/a").unwrap().slot + 1;
        let plugin_b_row = snapshot.plugin("github.com/acme/b").unwrap().slot + 1;

        let plugin_a_event = ProgressEvent::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl("https://example.com/a.git".to_string())),
        };
        apply_event(&mut snapshot, plugin_a_event.clone());
        let plugin_a_lines = transcript.render_lines(&snapshot, &plugin_a_event);
        renderer.write_reducer_lines(&snapshot, &plugin_a_event, plugin_a_lines);
        let plugin_a_line_after_a =
            renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();

        let plugin_b_event = ProgressEvent::PluginStageChanged {
            id: "github.com/acme/b".to_string(),
            stage: PluginStage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl("https://example.com/b.git".to_string())),
        };
        apply_event(&mut snapshot, plugin_b_event.clone());
        let plugin_b_lines = transcript.render_lines(&snapshot, &plugin_b_event);
        renderer.write_reducer_lines(&snapshot, &plugin_b_event, plugin_b_lines);

        assert_eq!(
            renderer.frame_line_for_tests(plugin_a_row).unwrap(),
            plugin_a_line_after_a.as_str()
        );
        assert!(renderer.frame_line_for_tests(plugin_b_row).unwrap().contains("plugin-b"));

        let finished_event = ProgressEvent::PluginFinished {
            id: "github.com/acme/a".to_string(),
            outcome: PluginOutcome::Installed { commit: "abc1234".to_string() },
        };
        apply_event(&mut snapshot, finished_event.clone());
        let finished_lines = transcript.render_lines(&snapshot, &finished_event);
        renderer.write_reducer_lines(&snapshot, &finished_event, finished_lines);
        let finished_line = renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();
        assert!(finished_line.contains("Installed"));

        let operation_event =
            ProgressEvent::OperationStageChanged { stage: OperationStage::WaitingForLock };
        apply_event(&mut snapshot, operation_event.clone());
        let operation_lines = transcript.render_lines(&snapshot, &operation_event);
        renderer.write_reducer_lines(&snapshot, &operation_event, operation_lines);
        assert_eq!(renderer.frame_line_for_tests(plugin_a_row).unwrap(), finished_line);

        renderer.finish(&snapshot, Some("update"), None);
        assert!(renderer.frozen_for_tests());
        let frozen_line = renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();

        let after_finish_event = ProgressEvent::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::Resolving,
            detail: None,
        };
        apply_event(&mut snapshot, after_finish_event.clone());
        let after_finish_lines = transcript.render_lines(&snapshot, &after_finish_event);
        renderer.write_reducer_lines(&snapshot, &after_finish_event, after_finish_lines);
        assert_eq!(renderer.frame_line_for_tests(plugin_a_row).unwrap(), frozen_line);
    }

    #[test]
    fn live_renderer_updates_target_row_without_overshoot() {
        let snapshot = ProgressSnapshot::from_ordered_plugins(vec![
            ("github.com/acme/a".to_string(), "plugin-a".to_string()),
            ("github.com/acme/b".to_string(), "plugin-b".to_string()),
        ]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);
        renderer.bootstrap(&snapshot);

        renderer.write_row(1, "updated-row".to_string());
        let output = renderer.output_for_tests();
        assert!(output.contains("\u{1b}[1A"), "output: {output:?}");
        assert!(output.contains("\u{1b}[1B"), "output: {output:?}");
        assert!(!output.contains("\u{1b}[2A"), "output: {output:?}");
        assert!(!output.contains("\u{1b}[2B"), "output: {output:?}");
    }
}
