use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{QueueableCommand, cursor};

use crate::progress::ACTION_WIDTH;
use crate::progress::log::ProgressIoAction;
use crate::progress::reducer::{ProgressSnapshot, SnapshotUpdate};
use crate::progress::render::DisplayLine;
use crate::termui::{self, Accent};

const DEFAULT_TERMINAL_WIDTH: usize = 120;

/// Live fixed-row renderer used by TTY reporter sinks.
pub(crate) struct LiveRenderer<W: Write> {
    writer: W,
    frame_lines: Vec<String>,
    io_failures: BTreeMap<ProgressIoAction, String>,
    terminal_width: usize,
    initialized: bool,
    frozen: bool,
}

impl<W: Write> LiveRenderer<W> {
    /// Create a live renderer using terminal width when available.
    ///
    /// Width is sampled once at construction time. tmup operations are typically
    /// short-lived, so we intentionally avoid resize tracking complexity.
    pub(crate) fn new(writer: W) -> Self {
        let terminal_width =
            terminal::size().map(|(width, _)| width as usize).unwrap_or(DEFAULT_TERMINAL_WIDTH);
        Self::new_with_width(writer, terminal_width)
    }

    fn new_with_width(writer: W, terminal_width: usize) -> Self {
        Self {
            writer,
            frame_lines: Vec::new(),
            io_failures: BTreeMap::new(),
            terminal_width: terminal_width.max(1),
            initialized: false,
            frozen: false,
        }
    }

    /// Reserve one operation row plus one row for each plugin entry.
    pub(crate) fn bootstrap(&mut self, snapshot: &ProgressSnapshot) {
        if self.frozen {
            return;
        }
        if !self.initialized {
            self.frame_lines = self.placeholder_frame(snapshot);
            if let Err(err) = self.writer.queue(cursor::Hide) {
                self.record_io_failure(ProgressIoAction::HideCursor, &err);
            }
            let initial_rows = self.frame_lines.clone();
            for line in initial_rows {
                self.try_writeln(ProgressIoAction::RowUpdate, &line);
            }
            self.try_flush();
            self.initialized = true;
            return;
        }

        let required_rows = 1 + snapshot.plugins.len();
        while self.frame_lines.len() < required_rows {
            let plugin_idx = self.frame_lines.len() - 1;
            let label = snapshot
                .plugins
                .get(plugin_idx)
                .map(|plugin| plugin.label.as_str())
                .unwrap_or("plugin");
            let line = self.placeholder_plugin_line(label);
            self.frame_lines.push(line.clone());
            self.try_writeln(ProgressIoAction::RowUpdate, &line);
        }
        self.try_flush();
    }

    /// Update the live frame for one reducer event.
    pub(crate) fn write_reducer_lines(
        &mut self,
        snapshot: &ProgressSnapshot,
        event: &SnapshotUpdate,
        lines: Vec<DisplayLine>,
    ) {
        if self.frozen {
            return;
        }
        self.bootstrap(snapshot);

        let Some(row) = row_for_event(snapshot, event) else {
            return;
        };
        debug_assert!(
            lines.len() <= 1,
            "live renderer expects at most one line per event; got {}",
            lines.len()
        );
        let Some(line) = lines.into_iter().next() else {
            return;
        };
        let rendered = termui::format_styled_labeled_line_clamped(
            &line.label,
            ACTION_WIDTH,
            &line.message,
            line.accent,
            self.render_width(),
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
            self.render_width(),
        );
        self.write_row(0, rendered);
    }

    /// Freeze the final frame and restore cursor state.
    pub(crate) fn finish(
        &mut self,
        snapshot: &ProgressSnapshot,
        command: Option<&'static str>,
        success: bool,
        details_path: Option<&Path>,
        warnings: &[String],
    ) {
        if self.frozen {
            return;
        }
        self.bootstrap(snapshot);

        if matches!(command, Some("init")) && success {
            let rendered = termui::format_styled_labeled_line_clamped(
                "Finished",
                ACTION_WIDTH,
                "tmup init",
                Accent::Success,
                self.render_width(),
            );
            self.write_row(0, rendered);
        }
        if let Some(path) = details_path {
            let details_line = termui::format_styled_labeled_line_clamped(
                "Details",
                ACTION_WIDTH,
                &path.display().to_string(),
                Accent::Muted,
                self.render_width(),
            );
            self.try_writeln(ProgressIoAction::RowUpdate, &details_line);
        }
        for warning in warnings {
            let warning_line = termui::format_styled_labeled_line_clamped(
                "Warning",
                ACTION_WIDTH,
                warning,
                Accent::Warning,
                self.render_width(),
            );
            self.try_writeln(ProgressIoAction::RowUpdate, &warning_line);
        }

        if let Err(err) = self.writer.queue(cursor::Show) {
            self.record_io_failure(ProgressIoAction::ShowCursor, &err);
        }
        self.try_flush();
        self.frozen = true;
    }

    fn placeholder_frame(&self, snapshot: &ProgressSnapshot) -> Vec<String> {
        let mut rows = Vec::with_capacity(1 + snapshot.plugins.len());
        rows.push(self.placeholder_operation_line());
        for plugin in &snapshot.plugins {
            rows.push(self.placeholder_plugin_line(&plugin.label));
        }
        rows
    }

    fn placeholder_operation_line(&self) -> String {
        termui::format_styled_labeled_line_clamped(
            "Status",
            ACTION_WIDTH,
            "pending",
            Accent::Muted,
            self.render_width(),
        )
    }

    fn placeholder_plugin_line(&self, label: &str) -> String {
        termui::format_styled_labeled_line_clamped(
            "Pending",
            ACTION_WIDTH,
            label,
            Accent::Muted,
            self.render_width(),
        )
    }

    fn render_width(&self) -> usize {
        self.terminal_width.saturating_sub(1).max(1)
    }

    fn write_row(&mut self, row: usize, rendered: String) {
        if row >= self.frame_lines.len() {
            return;
        }
        self.frame_lines[row] = rendered.clone();
        if !self.initialized {
            return;
        }

        let up = self.frame_lines.len().saturating_sub(row) as u16;
        if up > 0
            && let Err(err) = self.writer.queue(cursor::MoveUp(up))
        {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        if let Err(err) = self.writer.queue(cursor::MoveToColumn(0)) {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        if let Err(err) = self.writer.queue(Clear(ClearType::CurrentLine)) {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        if let Err(err) = self.writer.queue(Print(rendered)) {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        if up > 0
            && let Err(err) = self.writer.queue(cursor::MoveDown(up))
        {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        if let Err(err) = self.writer.queue(cursor::MoveToColumn(0)) {
            self.record_io_failure(ProgressIoAction::RowUpdate, &err);
        }
        self.try_flush();
    }

    fn record_io_failure(&mut self, action: ProgressIoAction, err: &std::io::Error) {
        self.io_failures.entry(action).or_insert_with(|| err.to_string());
    }

    fn try_writeln(&mut self, action: ProgressIoAction, line: &str) {
        if let Err(err) = writeln!(self.writer, "{line}") {
            self.record_io_failure(action, &err);
        }
    }

    fn try_flush(&mut self) {
        if let Err(err) = self.writer.flush() {
            self.record_io_failure(ProgressIoAction::Flush, &err);
        }
    }

    pub(crate) fn take_io_diagnostics(&mut self) -> Vec<(ProgressIoAction, String)> {
        std::mem::take(&mut self.io_failures).into_iter().collect()
    }
}

impl<W: Write> Drop for LiveRenderer<W> {
    fn drop(&mut self) {
        // Best-effort fallback for panic/poisoned-drop paths where reporter
        // cleanup may be skipped after the cursor has been hidden.
        if !self.initialized || self.frozen {
            return;
        }
        if let Err(err) = self.writer.queue(cursor::Show) {
            self.record_io_failure(ProgressIoAction::ShowCursor, &err);
        }
        self.try_flush();
        self.frozen = true;
    }
}

fn row_for_event(snapshot: &ProgressSnapshot, event: &SnapshotUpdate) -> Option<usize> {
    match event {
        SnapshotUpdate::OperationStageChanged { .. } | SnapshotUpdate::OperationFailed { .. } => {
            Some(0)
        }
        SnapshotUpdate::PluginStageChanged { id, .. }
        | SnapshotUpdate::PluginFinished { id, .. }
        | SnapshotUpdate::PluginFailed { id, .. } => snapshot.plugin_row(id),
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

    pub(crate) fn output_for_tests(&self) -> String {
        String::from_utf8_lossy(&self.writer).to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::io::Write;
    use std::rc::Rc;

    use unicode_width::UnicodeWidthStr;

    use super::LiveRenderer;
    use crate::progress::reducer::{ProgressSnapshot, SnapshotUpdate, apply_event};
    use crate::progress::render::{DisplayLine, LineKind, TranscriptRenderer};
    use crate::progress::{OperationStage, PluginOutcome, PluginStage, PluginStageDetail};
    use crate::termui::Accent;

    #[derive(Clone, Default)]
    struct SharedWriter {
        output: Rc<RefCell<Vec<u8>>>,
    }

    impl SharedWriter {
        fn new(output: Rc<RefCell<Vec<u8>>>) -> Self {
            Self { output }
        }
    }

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

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

        let plugin_a_row = snapshot.plugin_row("github.com/acme/a").unwrap();
        let plugin_b_row = snapshot.plugin_row("github.com/acme/b").unwrap();

        let plugin_a_event = SnapshotUpdate::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl("https://example.com/a.git".to_string())),
        };
        apply_event(&mut snapshot, &plugin_a_event);
        let plugin_a_lines = transcript.render_lines(&snapshot, &plugin_a_event);
        renderer.write_reducer_lines(&snapshot, &plugin_a_event, plugin_a_lines);
        let plugin_a_line_after_a =
            renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();

        let plugin_b_event = SnapshotUpdate::PluginStageChanged {
            id: "github.com/acme/b".to_string(),
            stage: PluginStage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl("https://example.com/b.git".to_string())),
        };
        apply_event(&mut snapshot, &plugin_b_event);
        let plugin_b_lines = transcript.render_lines(&snapshot, &plugin_b_event);
        renderer.write_reducer_lines(&snapshot, &plugin_b_event, plugin_b_lines);

        assert_eq!(
            renderer.frame_line_for_tests(plugin_a_row).unwrap(),
            plugin_a_line_after_a.as_str()
        );
        assert!(renderer.frame_line_for_tests(plugin_b_row).unwrap().contains("plugin-b"));

        let finished_event = SnapshotUpdate::PluginFinished {
            id: "github.com/acme/a".to_string(),
            outcome: PluginOutcome::Installed { commit: "abc1234".to_string() },
        };
        apply_event(&mut snapshot, &finished_event);
        let finished_lines = transcript.render_lines(&snapshot, &finished_event);
        renderer.write_reducer_lines(&snapshot, &finished_event, finished_lines);
        let finished_line = renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();
        assert!(finished_line.contains("Installed"));

        let operation_event =
            SnapshotUpdate::OperationStageChanged { stage: OperationStage::WaitingForLock };
        apply_event(&mut snapshot, &operation_event);
        let operation_lines = transcript.render_lines(&snapshot, &operation_event);
        renderer.write_reducer_lines(&snapshot, &operation_event, operation_lines);
        assert_eq!(renderer.frame_line_for_tests(plugin_a_row).unwrap(), finished_line);

        renderer.finish(&snapshot, Some("update"), true, None, &[]);
        assert!(renderer.frozen_for_tests());
        let frozen_line = renderer.frame_line_for_tests(plugin_a_row).unwrap().to_string();

        let after_finish_event = SnapshotUpdate::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::Resolving,
            detail: None,
        };
        apply_event(&mut snapshot, &after_finish_event);
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
        assert!(output.contains("\u{1b}[2A"), "output: {output:?}");
        assert!(output.contains("\u{1b}[2B"), "output: {output:?}");
        assert!(!output.contains("\u{1b}[1A"), "output: {output:?}");
        assert!(!output.contains("\u{1b}[1B"), "output: {output:?}");
    }

    #[test]
    fn live_renderer_bootstrap_expands_for_new_plugin_slots() {
        let mut snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);

        renderer.bootstrap(&snapshot);
        assert_eq!(renderer.frame_len_for_tests(), 2);

        snapshot.ensure_plugin("github.com/acme/b", "plugin-b");
        renderer.bootstrap(&snapshot);

        assert_eq!(renderer.frame_len_for_tests(), 3);
        assert!(renderer.frame_line_for_tests(2).unwrap().contains("plugin-b"));
    }

    #[test]
    fn live_renderer_finish_writes_details_and_restores_cursor() {
        let snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);
        let dir = tempfile::tempdir().unwrap();
        let details = dir.path().join("details.log");

        renderer.bootstrap(&snapshot);
        renderer.finish(&snapshot, Some("update"), true, Some(&details), &[]);

        let output = renderer.output_for_tests();
        assert!(output.contains("\u{1b}[?25l"), "output: {output:?}");
        assert!(output.contains("\u{1b}[?25h"), "output: {output:?}");
        assert!(output.contains("Details"), "output: {output:?}");
        assert!(output.contains(&details.display().to_string()), "output: {output:?}");
    }

    #[test]
    fn live_renderer_operation_failure_rewrites_operation_row() {
        let snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);

        renderer.bootstrap(&snapshot);
        renderer.write_operation_failure(&snapshot, "sync failed");

        let row = crate::progress::strip_ansi(renderer.frame_line_for_tests(0).unwrap());
        assert!(row.contains("Failed operation sync failed"), "row: {row:?}");
    }

    #[test]
    fn live_renderer_clamps_rows_for_narrow_widths() {
        let mut snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 12);
        let transcript = TranscriptRenderer::new();
        let event = SnapshotUpdate::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::CheckingOut,
            detail: None,
        };

        renderer.bootstrap(&snapshot);
        apply_event(&mut snapshot, &event);
        let lines = transcript.render_lines(&snapshot, &event);
        renderer.write_reducer_lines(&snapshot, &event, lines);

        let row = snapshot.plugin_row("github.com/acme/a").unwrap();
        let plain = crate::progress::strip_ansi(renderer.frame_line_for_tests(row).unwrap());
        assert!(UnicodeWidthStr::width(plain.as_str()) <= 12, "plain line: {plain:?}");
    }

    #[test]
    fn live_renderer_keeps_rows_strictly_within_terminal_width() {
        let mut snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 12);
        let transcript = TranscriptRenderer::new();
        let event = SnapshotUpdate::PluginStageChanged {
            id: "github.com/acme/a".to_string(),
            stage: PluginStage::CheckingOut,
            detail: None,
        };

        renderer.bootstrap(&snapshot);
        apply_event(&mut snapshot, &event);
        let lines = transcript.render_lines(&snapshot, &event);
        renderer.write_reducer_lines(&snapshot, &event, lines);

        let row = snapshot.plugin_row("github.com/acme/a").unwrap();
        let plain = crate::progress::strip_ansi(renderer.frame_line_for_tests(row).unwrap());
        assert!(
            UnicodeWidthStr::width(plain.as_str()) < 12,
            "live rows must leave one spare column to avoid terminal autowrap: {plain:?}"
        );
    }

    #[test]
    fn live_renderer_drop_restores_cursor_after_bootstrap_without_finish() {
        let output = Rc::new(RefCell::new(Vec::new()));
        let snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);

        {
            let mut renderer = LiveRenderer::new_with_width(SharedWriter::new(output.clone()), 120);
            renderer.bootstrap(&snapshot);
        }

        let output = String::from_utf8_lossy(output.borrow().as_slice()).to_string();
        assert!(output.contains("\u{1b}[?25l"), "output: {output:?}");
        assert!(output.contains("\u{1b}[?25h"), "output: {output:?}");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "live renderer expects at most one line per event")]
    fn live_renderer_panics_when_event_renders_multiple_lines() {
        let snapshot = ProgressSnapshot::from_ordered_plugins(vec![(
            "github.com/acme/a".to_string(),
            "plugin-a".to_string(),
        )]);
        let mut renderer = LiveRenderer::new_for_tests(Vec::new(), 120);
        let event = SnapshotUpdate::OperationStageChanged { stage: OperationStage::Syncing };
        let lines = vec![
            DisplayLine {
                kind: LineKind::Stage,
                accent: Accent::Info,
                label: "Syncing".to_string(),
                message: "remote plugins".to_string(),
            },
            DisplayLine {
                kind: LineKind::Stage,
                accent: Accent::Info,
                label: "Extra".to_string(),
                message: "line".to_string(),
            },
        ];

        renderer.write_reducer_lines(&snapshot, &event, lines);
    }
}
