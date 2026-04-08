use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::progress::catalog::DisplayCatalog;
use crate::progress::live::LiveRenderer;
use crate::progress::log::{DetailLog, ProgressIoAction};
use crate::progress::reducer::{self, ProgressSnapshot, SnapshotUpdate};
use crate::progress::render::{DisplayLine, TranscriptRenderer};
use crate::progress::{ACTION_WIDTH, ProgressEvent, ProgressReporter};
use crate::termui::{self, Accent};

struct TranscriptSink<W: Write> {
    writer: W,
    io_failures: BTreeMap<ProgressIoAction, String>,
}

impl<W: Write> TranscriptSink<W> {
    fn new(writer: W) -> Self {
        Self { writer, io_failures: BTreeMap::new() }
    }

    fn write_lines(&mut self, lines: Vec<DisplayLine>) {
        for line in lines {
            let rendered = line.styled(ACTION_WIDTH);
            self.try_writeln(ProgressIoAction::RowUpdate, &rendered);
        }
    }

    fn write_operation_failure(&mut self, summary: &str) {
        let rendered = termui::format_styled_labeled_line(
            "Failed",
            ACTION_WIDTH,
            &format!("operation {summary}"),
            Accent::Error,
        );
        self.try_writeln(ProgressIoAction::RowUpdate, &rendered);
    }

    fn write_init_finished(&mut self) {
        let rendered = termui::format_styled_labeled_line(
            "Finished",
            ACTION_WIDTH,
            "tmup init",
            Accent::Success,
        );
        self.try_writeln(ProgressIoAction::RowUpdate, &rendered);
    }

    fn write_details_path(&mut self, path: &Path) {
        let rendered = termui::format_styled_labeled_line(
            "Details",
            ACTION_WIDTH,
            &path.display().to_string(),
            Accent::Muted,
        );
        self.try_writeln(ProgressIoAction::RowUpdate, &rendered);
    }

    fn write_warning(&mut self, warning: &str) {
        let rendered =
            termui::format_styled_labeled_line("Warning", ACTION_WIDTH, warning, Accent::Warning);
        self.try_writeln(ProgressIoAction::RowUpdate, &rendered);
    }

    fn try_writeln(&mut self, action: ProgressIoAction, rendered: &str) {
        if let Err(err) = writeln!(self.writer, "{rendered}") {
            self.io_failures.entry(action).or_insert_with(|| err.to_string());
        }
    }

    fn take_io_diagnostics(&mut self) -> Vec<(ProgressIoAction, String)> {
        std::mem::take(&mut self.io_failures).into_iter().collect()
    }
}

enum ReporterSink<W: Write> {
    Transcript(TranscriptSink<W>),
    Live(LiveRenderer<W>),
}

struct ReporterState<W: Write> {
    sink: ReporterSink<W>,
    renderer: TranscriptRenderer,
    log: DetailLog,
    snapshot: ProgressSnapshot,
    recorded_io_actions: BTreeSet<ProgressIoAction>,
    deferred_warnings: Vec<String>,
    finished: bool,
}

impl<W: Write> ReporterState<W> {
    fn ensure_plugin_slot(&mut self, id: &str, name: &str) {
        self.snapshot.ensure_plugin(id, name);
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
        self.drain_sink_io_diagnostics();

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

        self.drain_sink_io_diagnostics();
    }

    fn drain_sink_io_diagnostics(&mut self) {
        let diagnostics = match &mut self.sink {
            ReporterSink::Transcript(sink) => sink.take_io_diagnostics(),
            ReporterSink::Live(renderer) => renderer.take_io_diagnostics(),
        };
        for (action, detail) in diagnostics {
            if self.recorded_io_actions.insert(action) {
                self.log.record_progress_io_diagnostic(action, &detail);
            }
        }
        if let Some(warning) = self.log.take_warning() {
            self.deferred_warnings.push(warning);
        }
    }
}

/// Private normalized event form used by the runtime reporter.
///
/// Public progress events borrow caller-owned data. The reporter converts them
/// once at the boundary into owned values and sanitized failure summaries, so
/// reducer/log/sink consumers share the same normalized payload.
enum NormalizedEvent {
    OperationStart,
    OperationStage {
        stage: crate::progress::OperationStage,
    },
    PluginStage {
        id: String,
        name: String,
        stage: crate::progress::PluginStage,
        detail: Option<crate::progress::PluginStageDetail>,
    },
    PluginFinished {
        id: String,
        name: String,
        outcome: crate::progress::PluginOutcome,
    },
    PluginFailed {
        id: String,
        name: String,
        stage: Option<crate::progress::PluginStage>,
        summary: String,
        detail: String,
        context: Vec<(&'static str, String)>,
    },
    OperationFailed {
        summary: String,
        detail: String,
    },
    OperationEnd {
        command: &'static str,
        success: bool,
    },
}

impl NormalizedEvent {
    fn from_public(event: ProgressEvent<'_>) -> Self {
        match event {
            ProgressEvent::OperationStart { .. } => Self::OperationStart,
            ProgressEvent::OperationStage { stage } => Self::OperationStage { stage },
            ProgressEvent::PluginStage { id, name, stage, detail } => {
                Self::PluginStage { id: id.to_string(), name: name.to_string(), stage, detail }
            }
            ProgressEvent::PluginFinished { id, name, outcome } => {
                Self::PluginFinished { id: id.to_string(), name: name.to_string(), outcome }
            }
            ProgressEvent::PluginFailed { id, name, stage, summary, detail, context } => {
                Self::PluginFailed {
                    id: id.to_string(),
                    name: name.to_string(),
                    stage,
                    summary: super::sanitize_summary(&summary, super::SUMMARY_MAX_LEN),
                    detail,
                    context,
                }
            }
            ProgressEvent::OperationFailed { summary, detail } => Self::OperationFailed {
                summary: super::sanitize_summary(&summary, super::SUMMARY_MAX_LEN),
                detail,
            },
            ProgressEvent::OperationEnd { command, success } => {
                Self::OperationEnd { command, success }
            }
        }
    }

    fn plugin_identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::PluginStage { id, name, .. }
            | Self::PluginFinished { id, name, .. }
            | Self::PluginFailed { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        }
    }

    fn snapshot_update(&self) -> Option<SnapshotUpdate> {
        match self {
            Self::OperationStart | Self::OperationEnd { .. } => None,
            Self::OperationStage { stage } => {
                Some(SnapshotUpdate::OperationStageChanged { stage: *stage })
            }
            Self::OperationFailed { summary, .. } => {
                Some(SnapshotUpdate::OperationFailed { summary: summary.clone() })
            }
            Self::PluginStage { id, stage, detail, .. } => {
                Some(SnapshotUpdate::PluginStageChanged {
                    id: id.clone(),
                    stage: *stage,
                    detail: detail.clone(),
                })
            }
            Self::PluginFinished { id, outcome, .. } => {
                Some(SnapshotUpdate::PluginFinished { id: id.clone(), outcome: outcome.clone() })
            }
            Self::PluginFailed { id, stage, summary, .. } => Some(SnapshotUpdate::PluginFailed {
                id: id.clone(),
                stage: *stage,
                summary: summary.clone(),
            }),
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
        Self::new_with_live_writer(logs_root, command, catalog, anstream::stderr())
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
        Self::new_with_sink(
            logs_root,
            command,
            catalog,
            ReporterSink::Transcript(TranscriptSink::new(writer)),
        )
    }

    fn new_with_live_writer(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        writer: W,
    ) -> Self {
        Self::new_with_sink(
            logs_root,
            command,
            catalog,
            ReporterSink::Live(LiveRenderer::new(writer)),
        )
    }

    fn new_with_sink(
        logs_root: &Path,
        command: &str,
        catalog: DisplayCatalog,
        sink: ReporterSink<W>,
    ) -> Self {
        let snapshot = snapshot_from_catalog(&catalog);
        Self {
            state: Mutex::new(ReporterState {
                sink,
                renderer: TranscriptRenderer::new(),
                log: DetailLog::new(logs_root, command),
                snapshot,
                recorded_io_actions: BTreeSet::new(),
                deferred_warnings: Vec::new(),
                finished: false,
            }),
        }
    }
}

impl<W: Write + Send> ProgressReporter for ReducerReporter<W> {
    fn report(&self, event: ProgressEvent<'_>) {
        let event = NormalizedEvent::from_public(event);
        let mut inner = self.lock_state();
        if inner.finished {
            return;
        }

        if let Some((id, name)) = event.plugin_identity() {
            inner.ensure_plugin_slot(id, name);
        }

        if let Some(snapshot_update) = event.snapshot_update() {
            inner.apply_snapshot_update(snapshot_update);
        }

        match event {
            NormalizedEvent::PluginFailed { id, name, stage, summary, detail, context } => {
                let context: Vec<(&str, &str)> =
                    context.iter().map(|(k, v)| (*k, v.as_str())).collect();
                inner.log.record_plugin_failure(&id, &name, stage, &summary, &detail, &context);
                if let Some(warning) = inner.log.take_warning() {
                    inner.deferred_warnings.push(warning);
                }
            }
            NormalizedEvent::OperationFailed { summary, detail } => {
                inner.log.record_operation_failure(&summary, &detail);
                if let Some(warning) = inner.log.take_warning() {
                    inner.deferred_warnings.push(warning);
                }
                inner.write_operation_failure(&summary);
            }
            NormalizedEvent::OperationEnd { command, success } => {
                inner.finish(Some(command), success);
            }
            _ => {}
        }
        inner.drain_sink_io_diagnostics();
    }
}

impl<W: Write + Send> Drop for ReducerReporter<W> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.state.lock() {
            inner.finish(None, false);
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
        Self::new_with_live_writer(logs_root, command, catalog, writer)
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
    use std::io::{self, Write};

    use super::ReducerReporter;
    use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};
    use crate::progress::catalog::DisplayCatalog;
    use crate::progress::reducer::OperationTerminalState;
    use crate::progress::{
        OperationStage, PluginStage, PluginStageDetail, ProgressEvent, ProgressReporter,
    };

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

    #[derive(Default)]
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("simulated transcript sink failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("simulated transcript sink flush failure"))
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
    fn reporter_records_operation_failure_in_snapshot() {
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
        reporter.report(ProgressEvent::OperationStage { stage: OperationStage::WaitingForLock });
        reporter.report(ProgressEvent::OperationFailed {
            summary: "sync failed".to_string(),
            detail: "full error".to_string(),
        });

        let (snapshot, _) = reporter.snapshot_and_output_for_tests();
        assert_eq!(snapshot.operation.stage, Some(OperationStage::WaitingForLock));
        assert!(matches!(
            snapshot.operation.terminal,
            Some(OperationTerminalState::Failed { ref summary }) if summary == "sync failed"
        ));
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
    fn reporter_logs_terminal_io_diagnostics_without_failing_operation() {
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
            ReducerReporter::new_with_writer(dir.path(), "update", catalog, FailingWriter);

        reporter.report(ProgressEvent::OperationStart { command: "update" });
        reporter.report(ProgressEvent::OperationStage { stage: OperationStage::WaitingForLock });
        reporter.report(ProgressEvent::OperationEnd { command: "update", success: true });
        drop(reporter);

        let log_file = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
            .expect("expected detail log with terminal I/O diagnostics");
        let log = std::fs::read_to_string(log_file.path()).unwrap();

        assert!(log.contains("== progress io action=row_update =="), "log: {log}");
        assert!(log.contains("terminal output write failure"), "log: {log}");
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
