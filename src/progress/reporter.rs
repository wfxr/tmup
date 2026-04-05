use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::progress::catalog::DisplayCatalog;
use crate::progress::live::LiveRenderer;
use crate::progress::log::DetailLog;
use crate::progress::reducer::{self, ProgressSnapshot, SnapshotUpdate};
use crate::progress::render::{DisplayLine, TranscriptRenderer};
use crate::progress::{ACTION_WIDTH, ProgressEvent, ProgressReporter};
use crate::termui::{self, Accent};

struct TranscriptSink<W: Write> {
    writer: W,
}

impl<W: Write> TranscriptSink<W> {
    fn new(writer: W) -> Self {
        Self { writer }
    }

    fn write_lines(&mut self, lines: Vec<DisplayLine>) {
        for line in lines {
            let rendered = line.styled(ACTION_WIDTH);
            let _ = writeln!(self.writer, "{rendered}");
        }
    }

    fn write_operation_failure(&mut self, summary: &str) {
        let rendered = termui::format_styled_labeled_line(
            "Failed",
            ACTION_WIDTH,
            &format!("operation {summary}"),
            Accent::Error,
        );
        let _ = writeln!(self.writer, "{rendered}");
    }

    fn write_init_finished(&mut self) {
        let rendered = termui::format_styled_labeled_line(
            "Finished",
            ACTION_WIDTH,
            "tmup init",
            Accent::Success,
        );
        let _ = writeln!(self.writer, "{rendered}");
    }

    fn write_details_path(&mut self, path: &Path) {
        let rendered = termui::format_styled_labeled_line(
            "Details",
            ACTION_WIDTH,
            &path.display().to_string(),
            Accent::Muted,
        );
        let _ = writeln!(self.writer, "{rendered}");
    }

    fn write_warning(&mut self, warning: &str) {
        let rendered =
            termui::format_styled_labeled_line("Warning", ACTION_WIDTH, warning, Accent::Warning);
        let _ = writeln!(self.writer, "{rendered}");
    }
}

enum ReporterSink<W: Write> {
    Transcript(TranscriptSink<W>),
    Live(LiveRenderer<W>),
}

enum SinkMode {
    Transcript,
    Live,
}

struct ReporterState<W: Write> {
    sink: ReporterSink<W>,
    renderer: TranscriptRenderer,
    log: DetailLog,
    snapshot: ProgressSnapshot,
    catalog: DisplayCatalog,
    deferred_warnings: Vec<String>,
    finished: bool,
}

impl<W: Write> ReporterState<W> {
    fn ensure_plugin_slot(&mut self, id: &str, name: &str) {
        let label = self.catalog.label_for(id, name).to_string();
        self.snapshot.ensure_plugin(id, &label);
    }

    fn apply_snapshot_update(&mut self, snapshot_update: SnapshotUpdate) {
        reducer::apply_event(&mut self.snapshot, &snapshot_update);
        let lines = self.renderer.render_lines(&self.snapshot, &snapshot_update);

        let snapshot = &self.snapshot;
        match &mut self.sink {
            ReporterSink::Transcript(sink) => sink.write_lines(lines),
            ReporterSink::Live(renderer) => {
                renderer.write_reducer_lines(snapshot, &snapshot_update, lines)
            }
        }
    }

    fn write_operation_failure(&mut self, summary: &str) {
        let snapshot = &self.snapshot;
        match &mut self.sink {
            ReporterSink::Transcript(sink) => sink.write_operation_failure(summary),
            ReporterSink::Live(renderer) => renderer.write_operation_failure(snapshot, summary),
        }
    }

    fn finish(&mut self, command: Option<&'static str>, success: bool) {
        if self.finished {
            return;
        }
        self.finished = true;

        let details_path = self.log.has_details().then(|| self.log.path().to_path_buf());
        let snapshot = &self.snapshot;
        let warnings = &self.deferred_warnings;

        match &mut self.sink {
            ReporterSink::Transcript(sink) => {
                if matches!(command, Some("init")) && success {
                    sink.write_init_finished();
                }
                if let Some(path) = details_path.as_deref() {
                    sink.write_details_path(path);
                }
                for warning in warnings {
                    sink.write_warning(warning);
                }
            }
            ReporterSink::Live(renderer) => {
                renderer.finish(snapshot, command, success, details_path.as_deref(), warnings)
            }
        }
    }
}

/// Runtime reducer-driven reporter implementation.
pub(crate) struct ReducerReporter<W: Write + Send> {
    state: Mutex<ReporterState<W>>,
}

impl ReducerReporter<anstream::AutoStream<std::io::Stderr>> {
    /// Create a reducer-driven reporter that writes to stderr.
    pub(crate) fn new(logs_root: &Path, command: &str, catalog: DisplayCatalog) -> Self {
        Self::new_with_writer(logs_root, command, catalog, anstream::stderr())
    }

    /// Create a reducer-driven reporter with the live TTY sink.
    pub(crate) fn new_live(logs_root: &Path, command: &str, catalog: DisplayCatalog) -> Self {
        Self::new_with_mode(logs_root, command, catalog, anstream::stderr(), SinkMode::Live)
    }
}

impl<W: Write + Send> ReducerReporter<W> {
    fn lock_state(&self) -> MutexGuard<'_, ReporterState<W>> {
        self.state.lock().expect("progress reporter state mutex poisoned")
    }

    fn new_with_writer(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        writer: W,
    ) -> Self {
        Self::new_with_mode(logs_root, command, catalog, writer, SinkMode::Transcript)
    }

    fn new_with_mode(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        writer: W,
        mode: SinkMode,
    ) -> Self {
        let snapshot = snapshot_from_catalog(&catalog);
        let sink = match mode {
            SinkMode::Transcript => ReporterSink::Transcript(TranscriptSink::new(writer)),
            SinkMode::Live => ReporterSink::Live(LiveRenderer::new(writer)),
        };
        Self {
            state: Mutex::new(ReporterState {
                sink,
                renderer: TranscriptRenderer::new(),
                log: DetailLog::new(logs_root, command),
                snapshot,
                catalog,
                deferred_warnings: Vec::new(),
                finished: false,
            }),
        }
    }

    fn finish(inner: &mut ReporterState<W>, command: Option<&'static str>, success: bool) {
        inner.finish(command, success);
    }

    /// Convert public borrowed progress events into an owned reducer event so the
    /// reducer can update snapshot state without borrowing the caller's payloads.
    /// `OperationStart` / `OperationEnd` stay in the public API for external
    /// reporters, but the reducer itself only needs state transitions and
    /// plugin-terminal events.
    fn to_snapshot_update(event: &ProgressEvent<'_>) -> Option<SnapshotUpdate> {
        match event {
            ProgressEvent::OperationStart { .. }
            | ProgressEvent::OperationFailed { .. }
            | ProgressEvent::OperationEnd { .. } => None,
            ProgressEvent::OperationStage { stage } => {
                Some(SnapshotUpdate::OperationStageChanged { stage: *stage })
            }
            ProgressEvent::PluginStage { id, stage, detail, .. } => {
                Some(SnapshotUpdate::PluginStageChanged {
                    id: id.to_string(),
                    stage: *stage,
                    detail: detail.clone(),
                })
            }
            ProgressEvent::PluginFinished { id, outcome, .. } => {
                Some(SnapshotUpdate::PluginFinished {
                    id: id.to_string(),
                    outcome: outcome.clone(),
                })
            }
            ProgressEvent::PluginFailed { id, stage, summary, .. } => {
                Some(SnapshotUpdate::PluginFailed {
                    id: id.to_string(),
                    stage: *stage,
                    summary: super::sanitize_summary(summary, super::SUMMARY_MAX_LEN),
                })
            }
        }
    }
}

impl<W: Write + Send> ProgressReporter for ReducerReporter<W> {
    fn report(&self, event: ProgressEvent<'_>) {
        let mut inner = self.lock_state();
        if inner.finished {
            return;
        }

        match &event {
            ProgressEvent::PluginStage { id, name, .. }
            | ProgressEvent::PluginFinished { id, name, .. }
            | ProgressEvent::PluginFailed { id, name, .. } => {
                inner.ensure_plugin_slot(id, name);
            }
            _ => {}
        }

        if let Some(snapshot_update) = Self::to_snapshot_update(&event) {
            inner.apply_snapshot_update(snapshot_update);
        }

        match &event {
            ProgressEvent::PluginFailed { id, name, stage, summary, detail, context } => {
                let summary = super::sanitize_summary(summary, super::SUMMARY_MAX_LEN);
                let context: Vec<(&str, &str)> =
                    context.iter().map(|(k, v)| (*k, v.as_str())).collect();
                inner.log.record_plugin_failure(id, name, *stage, &summary, detail, &context);
                if let Some(warning) = inner.log.take_warning() {
                    inner.deferred_warnings.push(warning);
                }
            }
            ProgressEvent::OperationFailed { summary, detail } => {
                let summary = super::sanitize_summary(summary, super::SUMMARY_MAX_LEN);
                inner.log.record_operation_failure(&summary, detail);
                if let Some(warning) = inner.log.take_warning() {
                    inner.deferred_warnings.push(warning);
                }
                inner.write_operation_failure(&summary);
            }
            ProgressEvent::OperationEnd { command, success } => {
                Self::finish(&mut inner, Some(command), *success);
            }
            _ => {}
        }
    }
}

impl<W: Write + Send> Drop for ReducerReporter<W> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.state.lock() {
            Self::finish(&mut inner, None, false);
        }
    }
}

fn snapshot_from_catalog(catalog: &DisplayCatalog) -> ProgressSnapshot {
    let plugins =
        catalog.iter().map(|plugin| (plugin.id.clone(), plugin.label.clone())).collect::<Vec<_>>();
    ProgressSnapshot::from_ordered_plugins(plugins)
}

#[cfg(test)]
impl ReducerReporter<Vec<u8>> {
    fn new_with_writer_for_tests(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        writer: Vec<u8>,
    ) -> Self {
        Self::new_with_writer(logs_root, command, catalog, writer)
    }

    fn snapshot_and_output_for_tests(&self) -> (ProgressSnapshot, String) {
        let state = self.state.lock().expect("progress reporter state mutex poisoned");
        let output = match &state.sink {
            ReporterSink::Transcript(sink) => String::from_utf8_lossy(&sink.writer).to_string(),
            ReporterSink::Live(_) => String::new(),
        };
        (state.snapshot.clone(), output)
    }

    fn new_live_with_writer_for_tests(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        writer: Vec<u8>,
    ) -> Self {
        Self::new_with_mode(logs_root, command, catalog, writer, SinkMode::Live)
    }

    fn snapshot_and_live_output_for_tests(&self) -> (ProgressSnapshot, String) {
        let state = self.state.lock().expect("progress reporter state mutex poisoned");
        let output = match &state.sink {
            ReporterSink::Live(renderer) => renderer.output_for_tests(),
            ReporterSink::Transcript(_) => String::new(),
        };
        (state.snapshot.clone(), output)
    }
}

#[cfg(test)]
mod tests {
    use super::ReducerReporter;
    use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};
    use crate::progress::catalog::DisplayCatalog;
    use crate::progress::model::PluginStageDetail;
    use crate::progress::{OperationStage, PluginStage, ProgressEvent, ProgressReporter};

    fn remote_plugin(raw: &str, id: &str, name: &str) -> PluginSpec {
        PluginSpec {
            source: PluginSource::Remote {
                raw: raw.to_string(),
                id: id.to_string(),
                clone_url: format!("https://{id}.git"),
            },
            name: name.to_string(),
            opt_prefix: "@plugin".to_string(),
            tracking: Tracking::DefaultBranch,
            build: None,
            opts: Vec::new(),
        }
    }

    #[test]
    fn reporter_finishes_once_and_logs_failures() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            options: Options::default(),
            plugins: vec![remote_plugin(
                "tmux-plugins/tmux-sensible",
                "github.com/tmux-plugins/tmux-sensible",
                "tmux-sensible",
            )],
        };
        let catalog = DisplayCatalog::from_config(&config, None);

        let reporter =
            ReducerReporter::new_with_writer_for_tests(dir.path(), "update", catalog, Vec::new());

        reporter.report(ProgressEvent::OperationStart { command: "update" });
        reporter.report(ProgressEvent::OperationStage { stage: OperationStage::WaitingForLock });
        reporter.report(ProgressEvent::PluginStage {
            id: "github.com/tmux-plugins/tmux-sensible",
            name: "tmux-sensible",
            stage: PluginStage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl(
                "https://github.com/tmux-plugins/tmux-sensible.git".to_string(),
            )),
        });
        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/tmux-plugins/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(PluginStage::Fetching),
            summary: "git fetch origin failed".to_string(),
            detail: "full error output".to_string(),
            context: vec![(
                "clone_url",
                "https://github.com/tmux-plugins/tmux-sensible.git".into(),
            )],
        });
        reporter.report(ProgressEvent::OperationEnd { command: "update", success: true });
        reporter.report(ProgressEvent::OperationEnd { command: "update", success: true });

        let (snapshot, output) = reporter.snapshot_and_output_for_tests();
        let output = crate::progress::strip_ansi(&output);
        assert!(matches!(snapshot.operation.stage, Some(OperationStage::WaitingForLock)));
        assert_eq!(snapshot.plugins.len(), 1);
        assert!(
            output.matches("Details ").count() == 1,
            "finish epilogue should be written exactly once, output:\n{output}"
        );

        let log_file = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .expect("log file should exist");
        let log = std::fs::read_to_string(log_file.path()).unwrap();
        assert!(log.contains("id=github.com/tmux-plugins/tmux-sensible"), "log: {log}");
        assert!(log.contains("name=tmux-sensible"), "log: {log}");
        assert!(log.contains("stage=fetching"), "log: {log}");
        assert!(
            log.contains("clone_url: https://github.com/tmux-plugins/tmux-sensible.git"),
            "log: {log}"
        );
    }

    #[test]
    fn reporter_preserves_catalog_slot_order() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            options: Options::default(),
            plugins: vec![
                remote_plugin("zed/tmux-z", "github.com/zed/tmux-z", "tmux-z"),
                remote_plugin("alpha/tmux-a", "github.com/alpha/tmux-a", "tmux-a"),
            ],
        };
        let catalog = DisplayCatalog::from_config(&config, None);
        let reporter =
            ReducerReporter::new_with_writer_for_tests(dir.path(), "update", catalog, Vec::new());

        let (snapshot, _) = reporter.snapshot_and_output_for_tests();
        let ordered_ids = snapshot.plugins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ordered_ids, vec!["github.com/zed/tmux-z", "github.com/alpha/tmux-a"]);
    }

    #[test]
    fn reporter_does_not_mark_failed_init_as_finished_in_transcript_output() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            options: Options::default(),
            plugins: vec![remote_plugin(
                "tmux-plugins/tmux-sensible",
                "github.com/tmux-plugins/tmux-sensible",
                "tmux-sensible",
            )],
        };
        let catalog = DisplayCatalog::from_config(&config, None);
        let reporter =
            ReducerReporter::new_with_writer_for_tests(dir.path(), "init", catalog, Vec::new());

        reporter.report(ProgressEvent::OperationStart { command: "init" });
        reporter.report(ProgressEvent::OperationFailed {
            summary: "sync failed".to_string(),
            detail: "full error".to_string(),
        });
        reporter.report(ProgressEvent::OperationEnd { command: "init", success: false });

        let (_, output) = reporter.snapshot_and_output_for_tests();
        let output = crate::progress::strip_ansi(&output);
        assert!(output.contains("Failed operation sync failed"), "output:\n{output}");
        assert!(!output.contains("Finished tmup init"), "output:\n{output}");
    }

    #[test]
    fn reporter_does_not_overwrite_failed_init_row_in_live_output() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            options: Options::default(),
            plugins: vec![remote_plugin(
                "tmux-plugins/tmux-sensible",
                "github.com/tmux-plugins/tmux-sensible",
                "tmux-sensible",
            )],
        };
        let catalog = DisplayCatalog::from_config(&config, None);
        let reporter = ReducerReporter::new_live_with_writer_for_tests(
            dir.path(),
            "init",
            catalog,
            Vec::new(),
        );

        reporter.report(ProgressEvent::OperationStart { command: "init" });
        reporter.report(ProgressEvent::OperationStage { stage: OperationStage::WaitingForLock });
        reporter.report(ProgressEvent::OperationFailed {
            summary: "sync failed".to_string(),
            detail: "full error".to_string(),
        });
        reporter.report(ProgressEvent::OperationEnd { command: "init", success: false });

        let (_, output) = reporter.snapshot_and_live_output_for_tests();
        let output = crate::progress::strip_ansi(&output);
        assert!(output.contains("Failed operation sync failed"), "output:\n{output}");
        assert!(!output.contains("Finished tmup init"), "output:\n{output}");
    }

    #[test]
    fn reporter_warns_when_detail_log_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let logs_root = dir.path().join("not-a-directory");
        std::fs::write(&logs_root, "occupied").unwrap();
        let config = Config {
            options: Options::default(),
            plugins: vec![remote_plugin(
                "tmux-plugins/tmux-sensible",
                "github.com/tmux-plugins/tmux-sensible",
                "tmux-sensible",
            )],
        };
        let catalog = DisplayCatalog::from_config(&config, None);
        let reporter =
            ReducerReporter::new_with_writer_for_tests(&logs_root, "update", catalog, Vec::new());

        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/tmux-plugins/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(PluginStage::Fetching),
            summary: "git fetch failed".to_string(),
            detail: "full error output".to_string(),
            context: vec![],
        });
        reporter.report(ProgressEvent::OperationEnd { command: "update", success: false });

        let (_, output) = reporter.snapshot_and_output_for_tests();
        let output = crate::progress::strip_ansi(&output);
        assert!(
            output.contains("Warning failed to write detail log"),
            "output should surface detail-log write failures:\n{output}"
        );
    }

    #[test]
    fn reporter_handles_empty_catalog_without_plugin_rows() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config { options: Options::default(), plugins: vec![] };
        let catalog = DisplayCatalog::from_config(&config, None);
        let reporter =
            ReducerReporter::new_with_writer_for_tests(dir.path(), "init", catalog, Vec::new());

        reporter.report(ProgressEvent::OperationStart { command: "init" });
        reporter.report(ProgressEvent::OperationStage { stage: OperationStage::WaitingForLock });
        reporter.report(ProgressEvent::OperationEnd { command: "init", success: true });

        let (snapshot, output) = reporter.snapshot_and_output_for_tests();
        let output = crate::progress::strip_ansi(&output);
        assert!(snapshot.plugins.is_empty(), "snapshot: {snapshot:?}");
        assert!(output.contains("Waiting lock"), "output:\n{output}");
        assert!(output.contains("Finished tmup init"), "output:\n{output}");
    }
}
