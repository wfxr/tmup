use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use crate::progress::catalog::DisplayCatalog;
use crate::progress::live::LiveRenderer;
use crate::progress::log::DetailLog;
use crate::progress::reducer::{self, ProgressSnapshot};
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
}

enum ReporterSink<W: Write> {
    Transcript(TranscriptSink<W>),
    Live(LiveRenderer<W>),
}

impl<W: Write> ReporterSink<W> {
    fn write_reducer_lines(
        &mut self,
        snapshot: &ProgressSnapshot,
        event: &reducer::ProgressEvent,
        lines: Vec<DisplayLine>,
    ) {
        match self {
            Self::Transcript(sink) => sink.write_lines(lines),
            Self::Live(renderer) => renderer.write_reducer_lines(snapshot, event, lines),
        }
    }

    fn write_operation_failure(&mut self, snapshot: &ProgressSnapshot, summary: &str) {
        match self {
            Self::Transcript(sink) => sink.write_operation_failure(summary),
            Self::Live(renderer) => renderer.write_operation_failure(snapshot, summary),
        }
    }

    fn finish(
        &mut self,
        snapshot: &ProgressSnapshot,
        command: Option<&'static str>,
        details_path: Option<&Path>,
    ) {
        match self {
            Self::Transcript(sink) => {
                if matches!(command, Some("init")) {
                    sink.write_init_finished();
                }
                if let Some(path) = details_path {
                    sink.write_details_path(path);
                }
            }
            Self::Live(renderer) => renderer.finish(snapshot, command, details_path),
        }
    }
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
    finished: bool,
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
                finished: false,
            }),
        }
    }

    fn finish(inner: &mut ReporterState<W>, command: Option<&'static str>) {
        if inner.finished {
            return;
        }
        inner.finished = true;
        let details_path = inner.log.has_details().then(|| inner.log.path().to_path_buf());
        inner.sink.finish(&inner.snapshot, command, details_path.as_deref());
    }

    fn to_reducer_event(event: &ProgressEvent<'_>) -> Option<reducer::ProgressEvent> {
        match event {
            ProgressEvent::OperationStart { .. }
            | ProgressEvent::OperationFailed { .. }
            | ProgressEvent::OperationEnd { .. } => None,
            ProgressEvent::OperationStage { stage } => {
                Some(reducer::ProgressEvent::OperationStageChanged { stage: *stage })
            }
            ProgressEvent::PluginStage { id, stage, detail, .. } => {
                Some(reducer::ProgressEvent::PluginStageChanged {
                    id: (*id).to_string(),
                    stage: *stage,
                    detail: detail.clone(),
                })
            }
            ProgressEvent::PluginFinished { id, outcome, .. } => {
                Some(reducer::ProgressEvent::PluginFinished {
                    id: (*id).to_string(),
                    outcome: outcome.clone(),
                })
            }
            ProgressEvent::PluginFailed { id, stage, summary, .. } => {
                Some(reducer::ProgressEvent::PluginFailed {
                    id: (*id).to_string(),
                    stage: *stage,
                    summary: super::sanitize_summary(summary, super::SUMMARY_MAX_LEN),
                })
            }
        }
    }
}

impl<W: Write + Send> ProgressReporter for ReducerReporter<W> {
    fn report(&self, event: ProgressEvent<'_>) {
        let mut inner = self.state.lock().unwrap();
        if inner.finished {
            return;
        }

        match &event {
            ProgressEvent::PluginStage { id, name, .. }
            | ProgressEvent::PluginFinished { id, name, .. }
            | ProgressEvent::PluginFailed { id, name, .. } => {
                let label = inner.catalog.label_for(id, name).to_string();
                inner.snapshot.ensure_plugin(id, &label);
            }
            _ => {}
        }

        if let Some(reducer_event) = Self::to_reducer_event(&event) {
            reducer::apply_event(&mut inner.snapshot, reducer_event.clone());
            let lines = inner.renderer.render_lines(&inner.snapshot, &reducer_event);
            let snapshot = inner.snapshot.clone();
            inner.sink.write_reducer_lines(&snapshot, &reducer_event, lines);
        }

        match &event {
            ProgressEvent::PluginFailed { id, name, stage, summary, detail, context } => {
                let summary = super::sanitize_summary(summary, super::SUMMARY_MAX_LEN);
                let context: Vec<(&str, &str)> =
                    context.iter().map(|(k, v)| (*k, v.as_str())).collect();
                inner.log.record_plugin_failure(id, name, *stage, &summary, detail, &context);
            }
            ProgressEvent::OperationFailed { summary, detail } => {
                let summary = super::sanitize_summary(summary, super::SUMMARY_MAX_LEN);
                inner.log.record_operation_failure(&summary, detail);
                let snapshot = inner.snapshot.clone();
                inner.sink.write_operation_failure(&snapshot, &summary);
            }
            ProgressEvent::OperationEnd { command } => {
                Self::finish(&mut inner, Some(command));
            }
            _ => {}
        }
    }
}

impl<W: Write + Send> Drop for ReducerReporter<W> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.state.lock() {
            Self::finish(&mut inner, None);
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
        let state = self.state.lock().unwrap();
        let output = match &state.sink {
            ReporterSink::Transcript(sink) => String::from_utf8_lossy(&sink.writer).to_string(),
            ReporterSink::Live(_) => String::new(),
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
    use crate::progress::{OperationStage, ProgressEvent, ProgressReporter, Stage};

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
            stage: Stage::Fetching,
            detail: Some(PluginStageDetail::CloneUrl(
                "https://github.com/tmux-plugins/tmux-sensible.git".to_string(),
            )),
        });
        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/tmux-plugins/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(Stage::Fetching),
            summary: "git fetch origin failed".to_string(),
            detail: "full error output".to_string(),
            context: vec![(
                "clone_url",
                "https://github.com/tmux-plugins/tmux-sensible.git".into(),
            )],
        });
        reporter.report(ProgressEvent::OperationEnd { command: "update" });
        reporter.report(ProgressEvent::OperationEnd { command: "update" });

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
}
