use std::collections::HashMap;
use std::io::IsTerminal;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
use log::DetailLog;
use model::{PluginOutcome, PluginStageDetail};
#[cfg(test)]
use model::{SkipReason, TrackingResolution, TrackingSelector};
#[cfg(test)]
use owo_colors::OwoColorize;

use crate::model::Config;
use crate::state::Paths;
#[cfg(test)]
use crate::termui;
#[cfg(test)]
use crate::termui::Accent;

/// Stable plugin display-catalog structures for structured progress.
pub mod catalog;
/// Live fixed-row progress renderer for TTY output.
pub(crate) mod live;
/// Shared failure-detail logging primitives.
pub(crate) mod log;
/// Structured progress event/value types for reducer/renderer evolution.
pub mod model;
/// Deterministic reducer and snapshot state for structured progress.
pub mod reducer;
/// Shared progress line rendering from structured snapshot state.
pub(crate) mod render;
/// Reducer-driven runtime reporter core.
pub(crate) mod reporter;

#[allow(unused_imports)]
pub(crate) use catalog::{DisplayCatalog, DisplayPlugin};
pub use model::{OperationStage, PluginStage as Stage};
#[allow(unused_imports)]
pub(crate) use reducer::{
    PluginDisplayState as StructuredPluginDisplayState, PluginSnapshot as StructuredPluginSnapshot,
    ProgressEvent as StructuredProgressEvent, ProgressSnapshot as StructuredProgressSnapshot,
    apply_event as apply_structured_event,
};

const SUMMARY_MAX_LEN: usize = 80;
const ACTION_WIDTH: usize = 12;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Progress events emitted during command execution.
pub enum ProgressEvent<'a> {
    /// The named command has started.
    OperationStart {
        /// The subcommand name (e.g. `"init"`, `"update"`).
        command: &'static str,
    },
    /// The overall operation has advanced to a new stage.
    OperationStage {
        /// The new operation stage.
        stage: OperationStage,
    },
    /// A plugin has advanced to a new processing stage.
    PluginStage {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// The new plugin stage.
        stage: Stage,
        /// Optional stage-specific structured detail.
        detail: Option<PluginStageDetail>,
    },
    /// A plugin reached a terminal outcome.
    PluginFinished {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// Structured completion outcome.
        outcome: PluginOutcome,
    },
    /// A plugin failed with a short summary and full diagnostic detail.
    PluginFailed {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// The processing stage at which the failure occurred.
        stage: Option<Stage>,
        /// One-line failure summary suitable for terminal output.
        summary: String,
        /// Full diagnostic text written to the log file.
        detail: String,
        /// Extra key-value context written to the log file.
        context: Vec<(&'static str, String)>,
    },
    /// The overall operation failed with a short summary and full diagnostic detail.
    OperationFailed {
        /// One-line failure summary suitable for terminal output.
        summary: String,
        /// Full diagnostic text written to the log file.
        detail: String,
    },
    /// The named command has completed.
    OperationEnd {
        /// The subcommand name (e.g. `"init"`, `"update"`).
        command: &'static str,
    },
}

/// Trait for receiving progress events.
pub trait ProgressReporter: Send + Sync {
    /// Deliver a single progress event to this reporter.
    fn report(&self, event: ProgressEvent<'_>);
}

/// An error that carries a user-facing progress message.
#[derive(Debug)]
pub struct ProgressFailure {
    message: String,
}

impl ProgressFailure {
    /// Create a new `ProgressFailure` with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for ProgressFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProgressFailure {}

/// A sentinel error indicating failure was already reported via progress output.
#[derive(Debug)]
pub struct ReportedError;

impl std::fmt::Display for ReportedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation failed (see progress output)")
    }
}

impl std::error::Error for ReportedError {}

/// Wrap a message into a `ProgressFailure` error.
pub fn progress_failure(message: impl Into<String>) -> anyhow::Error {
    ProgressFailure::new(message).into()
}

/// Construct a `ReportedError` as an `anyhow::Error`.
pub fn reported_error() -> anyhow::Error {
    ReportedError.into()
}

/// Return `true` if any cause in the error chain is a `ProgressFailure`.
pub fn is_progress_failure(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<ProgressFailure>().is_some())
}

/// Return `true` if any cause in the error chain is a `ReportedError`.
pub fn is_reported_error(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<ReportedError>().is_some())
}

/// Create a stream-oriented reporter for human-readable progress output.
pub fn create_reporter(
    paths: &Paths,
    command: &'static str,
    config: &Config,
    target_id: Option<&str>,
) -> Box<dyn ProgressReporter> {
    let catalog = DisplayCatalog::from_config(config, target_id);
    if std::io::stderr().is_terminal() {
        Box::new(reporter::ReducerReporter::new_live(&paths.logs_root, command, catalog))
    } else {
        Box::new(reporter::ReducerReporter::new(&paths.logs_root, command, catalog))
    }
}

/// Build stable display labels before progress output begins.
pub fn build_display_labels(config: &Config, target_id: Option<&str>) -> HashMap<String, String> {
    let catalog = DisplayCatalog::from_config(config, target_id);
    catalog.iter().map(|plugin| (plugin.id.clone(), plugin.label.clone())).collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip ANSI escape sequences and non-printable control characters.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
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
        if !ch.is_control() {
            result.push(ch);
        }
    }
    result
}

/// Sanitize a string: strip ANSI, collapse whitespace into a single line.
fn sanitize(s: &str) -> String {
    strip_ansi(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sanitize and truncate to `max_chars`.
fn sanitize_summary(s: &str, max_chars: usize) -> String {
    let clean = sanitize(s);
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let truncated: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

/// Extract a one-line summary and full detail from an anyhow error.
pub fn summarize_error(err: &anyhow::Error) -> (String, String) {
    let detail = format!("{err:?}");
    let summary = sanitize_summary(&format!("{err}"), SUMMARY_MAX_LEN);
    (summary, detail)
}

#[cfg(test)]
fn format_progress_line(action: &str, message: &str) -> String {
    termui::format_plain_labeled_line(action, ACTION_WIDTH, message)
}

/// Style reference tokens like `branch@master`, `commit@abc123` by
/// highlighting the value (after `@`) in magenta.
#[cfg(test)]
fn style_ref_tokens(s: &str) -> String {
    s.split(' ')
        .map(|token| {
            if let Some((prefix, value)) = token.split_once('@') {
                format!("{prefix}@{}", value.magenta())
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn operation_message(stage: OperationStage) -> &'static str {
    match stage {
        OperationStage::WaitingForLock => "lock",
        OperationStage::Syncing => "remote plugins",
        OperationStage::ApplyingWrites => "plugin contents",
        OperationStage::LoadingTmux => "applying load plan",
    }
}

#[cfg(test)]
fn format_operation_failure(summary: &str) -> String {
    format!("operation {}", sanitize_summary(summary, SUMMARY_MAX_LEN))
}

#[cfg(test)]
fn tracking_selector_text(selector: &TrackingSelector) -> String {
    match selector {
        TrackingSelector::DefaultBranch => "default-branch".to_string(),
        TrackingSelector::Branch(branch) => format!("branch@{branch}"),
        TrackingSelector::Tag(tag) => format!("tag@{tag}"),
        TrackingSelector::Commit(commit) => format!("commit@{commit}"),
    }
}

#[cfg(test)]
fn tracking_detail_text(
    selector: &TrackingSelector,
    resolved: &TrackingResolution,
    commit: &str,
) -> String {
    match (selector, resolved) {
        (TrackingSelector::DefaultBranch, TrackingResolution::DefaultBranch { branch })
        | (TrackingSelector::DefaultBranch, TrackingResolution::Branch { branch }) => {
            format!("default-branch -> branch@{branch} -> commit@{commit}")
        }
        (TrackingSelector::Branch(branch), _) => format!("branch@{branch} -> commit@{commit}"),
        (TrackingSelector::Tag(tag), _) => format!("tag@{tag} -> commit@{commit}"),
        (TrackingSelector::Commit(commit), _) => format!("commit@{commit}"),
        _ => format!(
            "{} -> {}",
            tracking_selector_text(selector),
            match resolved {
                TrackingResolution::DefaultBranch { branch }
                | TrackingResolution::Branch { branch } => {
                    format!("branch@{branch} -> commit@{commit}")
                }
                TrackingResolution::Tag { tag } => format!("tag@{tag} -> commit@{commit}"),
                TrackingResolution::Commit { commit } => format!("commit@{commit}"),
            }
        ),
    }
}

#[cfg(test)]
fn plugin_outcome_action_and_message(
    label: &str,
    outcome: &PluginOutcome,
) -> (&'static str, String) {
    match outcome {
        PluginOutcome::Installed { commit } => ("Installed", format!("{label} commit@{commit}")),
        PluginOutcome::Updated { from, to } => {
            ("Updated", format!("{label} commit@{from} -> commit@{to}"))
        }
        PluginOutcome::Synced { commit } => ("Synced", format!("{label} commit@{commit}")),
        PluginOutcome::Restored { commit } => ("Restored", format!("{label} commit@{commit}")),
        PluginOutcome::Reconciled => ("Reconciled", label.to_string()),
        PluginOutcome::CheckedUpToDate | PluginOutcome::AlreadyRestored => {
            ("Checked", label.to_string())
        }
        PluginOutcome::Skipped { reason } => {
            let reason = match reason {
                SkipReason::PinnedTag { tag } => format!("pinned to tag {tag}"),
                SkipReason::PinnedCommit { commit } => format!("pinned to commit {commit}"),
                SkipReason::KnownFailure { commit } => format!("known build failure at {commit}"),
                SkipReason::Other(reason) => reason.clone(),
            };
            ("Skipped", format!("{label} {reason}"))
        }
    }
}

#[cfg(test)]
fn title_case<T: std::fmt::Display>(value: T) -> String {
    let value = value.to_string();
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// NullReporter
// ---------------------------------------------------------------------------

/// A no-op reporter that silently discards all progress events.
pub struct NullReporter;

impl ProgressReporter for NullReporter {
    fn report(&self, _event: ProgressEvent<'_>) {}
}

// ---------------------------------------------------------------------------
// Stream renderer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum LineKind {
    Header,
    Stage,
    Success,
    Failure,
    Muted,
}

#[cfg(test)]
struct RenderedLine {
    kind: LineKind,
    action: String,
    message: String,
}

#[cfg(test)]
impl RenderedLine {
    fn new(kind: LineKind, action: impl Into<String>, message: impl Into<String>) -> Self {
        Self { kind, action: action.into(), message: message.into() }
    }

    #[cfg(test)]
    fn plain(&self) -> String {
        let message = strip_ansi(&self.message);
        if self.action.is_empty() { message } else { format_progress_line(&self.action, &message) }
    }

    fn styled(&self) -> String {
        if self.kind == LineKind::Header {
            termui::bold(&self.message)
        } else if self.action.is_empty() {
            self.message.clone()
        } else {
            termui::format_styled_labeled_line(
                &self.action,
                ACTION_WIDTH,
                &self.message,
                self.kind.into(),
            )
        }
    }
}

#[cfg(test)]
struct StreamRenderer {
    labels: HashMap<String, String>,
}

#[cfg(test)]
impl StreamRenderer {
    fn new(labels: HashMap<String, String>) -> Self {
        Self { labels }
    }

    #[cfg(test)]
    fn render(&self, event: ProgressEvent<'_>) -> Vec<String> {
        self.render_lines(event).into_iter().map(|line| line.plain()).collect()
    }

    fn render_lines(&self, event: ProgressEvent<'_>) -> Vec<RenderedLine> {
        match event {
            ProgressEvent::OperationStart { .. } => Vec::new(),
            ProgressEvent::OperationStage { stage } => match stage {
                OperationStage::WaitingForLock => vec![RenderedLine::new(
                    LineKind::Stage,
                    title_case(stage),
                    operation_message(stage),
                )],
                _ => Vec::new(),
            },
            ProgressEvent::PluginStage { id, name, stage, detail } => match stage {
                Stage::CheckingOut => Vec::new(),
                Stage::Applying if detail.is_none() => Vec::new(),
                _ => {
                    let action = match stage {
                        Stage::Applying => "Building".to_string(),
                        _ => title_case(stage),
                    };
                    vec![RenderedLine::new(
                        LineKind::Stage,
                        action,
                        self.stage_message(id, name, stage, detail.as_ref()),
                    )]
                }
            },
            ProgressEvent::PluginFinished { id, name, outcome } => {
                let label = self.label(id, name);
                let (action, message) = plugin_outcome_action_and_message(label, &outcome);
                vec![RenderedLine::new(LineKind::Success, action, message)]
            }
            ProgressEvent::PluginFailed { id, name, summary, .. } => vec![RenderedLine::new(
                LineKind::Failure,
                "Failed",
                format!("{} {}", self.label(id, name), sanitize_summary(&summary, SUMMARY_MAX_LEN)),
            )],
            ProgressEvent::OperationFailed { summary, .. } => {
                vec![RenderedLine::new(
                    LineKind::Failure,
                    "Failed",
                    format_operation_failure(&summary),
                )]
            }
            ProgressEvent::OperationEnd { command: "init" } => {
                vec![RenderedLine::new(LineKind::Success, "Finished", "tmup init")]
            }
            ProgressEvent::OperationEnd { .. } => Vec::new(),
        }
    }

    fn label<'a>(&'a self, id: &'a str, name: &'a str) -> &'a str {
        self.labels.get(id).map(String::as_str).unwrap_or(name)
    }

    fn stage_message(
        &self,
        id: &str,
        name: &str,
        stage: Stage,
        detail: Option<&PluginStageDetail>,
    ) -> String {
        let label = self.label(id, name);
        match (stage, detail) {
            (Stage::Cloning | Stage::Fetching, Some(PluginStageDetail::CloneUrl(url))) => {
                format!("{label} {}", url.blue())
            }
            (
                Stage::Resolving,
                Some(PluginStageDetail::TrackingResolution { selector, resolved, commit }),
            ) => {
                let detail = tracking_detail_text(selector, resolved, commit);
                format!("{label} {}", style_ref_tokens(&detail))
            }
            (Stage::Applying, Some(PluginStageDetail::BuildCommand(build_cmd))) => {
                format!("{label} {}", sanitize_summary(build_cmd, SUMMARY_MAX_LEN))
            }
            _ => label.to_string(),
        }
    }
}

#[cfg(test)]
impl From<LineKind> for Accent {
    fn from(value: LineKind) -> Self {
        match value {
            LineKind::Header => Accent::Bold,
            LineKind::Stage => Accent::Info,
            LineKind::Success => Accent::Success,
            LineKind::Failure => Accent::Error,
            LineKind::Muted => Accent::Muted,
        }
    }
}

// ---------------------------------------------------------------------------
// Stream reporter
// ---------------------------------------------------------------------------

#[cfg(test)]
struct StreamReporter<W: Write + Send> {
    state: Mutex<StreamReporterInner<W>>,
}

#[cfg(test)]
struct StreamReporterInner<W: Write> {
    writer: W,
    renderer: StreamRenderer,
    log: DetailLog,
    finished: bool,
}

#[cfg(test)]
impl<W: Write + Send> StreamReporter<W> {
    fn new_with_writer(
        logs_root: &Path,
        command: &str,
        labels: HashMap<String, String>,
        writer: W,
    ) -> Self {
        Self {
            state: Mutex::new(StreamReporterInner {
                writer,
                renderer: StreamRenderer::new(labels),
                log: DetailLog::new(logs_root, command),
                finished: false,
            }),
        }
    }

    fn finish(inner: &mut StreamReporterInner<W>) {
        if inner.finished {
            return;
        }
        inner.finished = true;
        if inner.log.has_details() {
            let line = RenderedLine::new(
                LineKind::Muted,
                "Details",
                inner.log.path().display().to_string(),
            );
            let _ = writeln!(inner.writer, "{}", line.styled());
        }
    }
}

#[cfg(test)]
impl<W: Write + Send> ProgressReporter for StreamReporter<W> {
    fn report(&self, event: ProgressEvent<'_>) {
        let mut inner = self.state.lock().unwrap();
        let should_finish = matches!(event, ProgressEvent::OperationEnd { .. });
        match &event {
            ProgressEvent::PluginFailed { id, name, stage, summary, detail, context } => {
                let summary = sanitize_summary(summary, SUMMARY_MAX_LEN);
                let ctx: Vec<(&str, &str)> =
                    context.iter().map(|(k, v)| (*k, v.as_str())).collect();
                inner.log.record_plugin_failure(id, name, *stage, &summary, detail, &ctx);
            }
            ProgressEvent::OperationFailed { summary, detail } => {
                let summary = sanitize_summary(summary, SUMMARY_MAX_LEN);
                inner.log.record_operation_failure(&summary, detail);
            }
            _ => {}
        }

        for line in inner.renderer.render_lines(event) {
            let _ = writeln!(inner.writer, "{}", line.styled());
        }

        if should_finish {
            Self::finish(&mut inner);
        }
    }
}

#[cfg(test)]
impl<W: Write + Send> Drop for StreamReporter<W> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.state.lock() {
            Self::finish(&mut inner);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};

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
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("\x1b[1;33;40mworld\x1b[0m"), "world");
    }

    #[test]
    fn strip_ansi_removes_control_chars() {
        assert_eq!(strip_ansi("hello\x07world"), "helloworld");
    }

    #[test]
    fn strip_ansi_preserves_unicode() {
        assert_eq!(strip_ansi("hello \u{1f600} world"), "hello \u{1f600} world");
    }

    #[test]
    fn sanitize_collapses_whitespace() {
        assert_eq!(sanitize("  hello   world\n  foo  "), "hello world foo");
    }

    #[test]
    fn sanitize_summary_truncates() {
        let long = "a".repeat(100);
        let result = sanitize_summary(&long, 10);
        assert_eq!(result.chars().count(), 10);
        assert!(result.ends_with('\u{2026}'));
    }

    #[test]
    fn sanitize_summary_short_string_unchanged() {
        assert_eq!(sanitize_summary("hello", 80), "hello");
    }

    #[test]
    fn null_reporter_accepts_events() {
        let r = NullReporter;
        r.report(ProgressEvent::OperationStart { command: "test" });
        r.report(ProgressEvent::OperationEnd { command: "test" });
    }

    #[test]
    fn summarize_error_produces_single_line() {
        let err = anyhow::anyhow!("line1\nline2\nline3");
        let (summary, detail) = summarize_error(&err);
        assert!(!summary.contains('\n'));
        assert!(detail.contains("line1"));
    }

    #[test]
    fn build_display_labels_prefers_name_until_collision() {
        let config = Config {
            options: Options::default(),
            plugins: vec![
                remote_plugin(
                    "tmux-plugins/tmux-sensible",
                    "github.com/tmux-plugins/tmux-sensible",
                    "tmux-sensible",
                ),
                remote_plugin(
                    "acme/tmux-sensible",
                    "github.com/acme/tmux-sensible",
                    "tmux-sensible",
                ),
                remote_plugin(
                    "tmux-plugins/tmux-yank",
                    "github.com/tmux-plugins/tmux-yank",
                    "tmux-yank",
                ),
            ],
        };

        let labels = build_display_labels(&config, None);

        assert_eq!(labels["github.com/tmux-plugins/tmux-sensible"], "tmux-plugins/tmux-sensible");
        assert_eq!(labels["github.com/acme/tmux-sensible"], "acme/tmux-sensible");
        assert_eq!(labels["github.com/tmux-plugins/tmux-yank"], "tmux-yank");
    }

    #[test]
    fn format_progress_line_right_aligns_action_column() {
        let line = format_progress_line("Updated", "tmux-sensible  abc1234 -> def5678");
        assert_eq!(line, "     Updated tmux-sensible  abc1234 -> def5678");
    }

    #[test]
    fn stream_renderer_renders_plain_event_flow() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());

        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::OperationStart { command: "update" }),
            Vec::<String>::new()
        );
        assert_eq!(
            renderer.render(ProgressEvent::OperationStage { stage: OperationStage::Syncing }),
            Vec::<String>::new()
        );
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Fetching,
                detail: None,
            }),
            vec!["    Fetching tmux-sensible".to_string()]
        );
        assert_eq!(
            renderer.render(ProgressEvent::PluginFinished {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                outcome: PluginOutcome::Updated {
                    from: "abc1234".to_string(),
                    to: "def5678".to_string(),
                },
            }),
            vec!["     Updated tmux-sensible commit@abc1234 -> commit@def5678".to_string()]
        );
        assert_eq!(
            renderer.render(ProgressEvent::OperationEnd { command: "update" }),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stream_renderer_uses_building_for_apply_stage() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Applying,
                detail: Some(PluginStageDetail::BuildCommand("make build".to_string())),
            }),
            vec!["    Building tmux-sensible make build".to_string()]
        );
    }

    #[test]
    fn stream_renderer_formats_synced_commit_without_extra_gap() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::PluginFinished {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                outcome: PluginOutcome::Synced { commit: "8c1eeec".to_string() },
            }),
            vec!["      Synced tmux-sensible commit@8c1eeec".to_string()]
        );
    }

    #[test]
    fn stream_renderer_skips_apply_stage_without_build_detail() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Applying,
                detail: None,
            }),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stream_renderer_skips_loading_tmux_stage_output() {
        let renderer = StreamRenderer::new(HashMap::new());
        assert_eq!(
            renderer.render(ProgressEvent::OperationStage { stage: OperationStage::LoadingTmux }),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stream_renderer_skips_checkout_stage_output() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::CheckingOut,
                detail: None,
            }),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stream_renderer_shows_clone_url_for_fetching_stage() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Fetching,
                detail: Some(PluginStageDetail::CloneUrl(
                    "https://github.com/tmux-plugins/tmux-sensible".to_string(),
                )),
            }),
            vec![
                "    Fetching tmux-sensible https://github.com/tmux-plugins/tmux-sensible"
                    .to_string()
            ]
        );
    }

    #[test]
    fn stream_renderer_shows_tag_resolution_detail() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Resolving,
                detail: Some(PluginStageDetail::TrackingResolution {
                    selector: TrackingSelector::Tag("v1.0".to_string()),
                    resolved: TrackingResolution::Tag { tag: "v1.0".to_string() },
                    commit: "8c1eeec".to_string(),
                }),
            }),
            vec!["   Resolving tmux-sensible tag@v1.0 -> commit@8c1eeec".to_string()]
        );
    }

    #[test]
    fn stream_renderer_shows_branch_resolution_detail() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Resolving,
                detail: Some(PluginStageDetail::TrackingResolution {
                    selector: TrackingSelector::Branch("main".to_string()),
                    resolved: TrackingResolution::Branch { branch: "main".to_string() },
                    commit: "8c1eeec".to_string(),
                }),
            }),
            vec!["   Resolving tmux-sensible branch@main -> commit@8c1eeec".to_string()]
        );
    }

    #[test]
    fn stream_renderer_shows_default_branch_resolution_detail() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);
        assert_eq!(
            renderer.render(ProgressEvent::PluginStage {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: Stage::Resolving,
                detail: Some(PluginStageDetail::TrackingResolution {
                    selector: TrackingSelector::DefaultBranch,
                    resolved: TrackingResolution::DefaultBranch { branch: "main".to_string() },
                    commit: "8c1eeec".to_string(),
                }),
            }),
            vec![
                "   Resolving tmux-sensible default-branch -> branch@main -> commit@8c1eeec"
                    .to_string()
            ]
        );
    }

    #[test]
    fn stream_renderer_uses_single_space_for_skipped_messages() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::PluginFinished {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                outcome: PluginOutcome::Skipped {
                    reason: SkipReason::PinnedTag { tag: "v1.0.0".to_string() },
                },
            }),
            vec!["     Skipped tmux-sensible pinned to tag v1.0.0".to_string()]
        );
    }

    #[test]
    fn stream_renderer_uses_single_space_for_failed_messages() {
        let mut labels = HashMap::new();
        labels.insert("github.com/tmux-plugins/tmux-sensible".to_string(), "tmux-sensible".into());
        let renderer = StreamRenderer::new(labels);

        assert_eq!(
            renderer.render(ProgressEvent::PluginFailed {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                stage: None,
                summary: "git clone --bare failed".to_string(),
                detail: String::new(),
                context: vec![],
            }),
            vec!["      Failed tmux-sensible git clone --bare failed".to_string()]
        );
    }

    #[test]
    fn stream_renderer_uses_single_space_for_operation_failures() {
        let renderer = StreamRenderer::new(HashMap::new());

        assert_eq!(
            renderer.render(ProgressEvent::OperationFailed {
                summary: "failed to write lockfile".to_string(),
                detail: String::new(),
            }),
            vec!["      Failed operation failed to write lockfile".to_string()]
        );
    }

    #[test]
    fn detail_log_includes_canonical_plugin_identity_and_stage() {
        let dir = tempfile::tempdir().unwrap();
        let reporter =
            StreamReporter::new_with_writer(dir.path(), "test", HashMap::new(), Vec::new());
        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/tmux-plugins/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(Stage::Fetching),
            summary: "git fetch origin failed".to_string(),
            detail: "full error output here".to_string(),
            context: vec![
                ("clone_url", "https://github.com/tmux-plugins/tmux-sensible.git".to_string()),
                ("tracking", "default-branch".to_string()),
            ],
        });
        drop(reporter);

        let log_file = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .expect("log file should be created");
        let log = std::fs::read_to_string(log_file.path()).unwrap();

        assert!(log.contains("id=github.com/tmux-plugins/tmux-sensible"), "log: {log}");
        assert!(log.contains("name=tmux-sensible"), "log: {log}");
        assert!(log.contains("stage=fetching"), "log: {log}");
        assert!(log.contains("summary: git fetch origin failed"), "log: {log}");
        assert!(
            log.contains("clone_url: https://github.com/tmux-plugins/tmux-sensible.git"),
            "log: {log}"
        );
        assert!(log.contains("tracking: default-branch"), "log: {log}");
    }

    #[test]
    fn detail_log_differentiates_same_name_plugins_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let reporter =
            StreamReporter::new_with_writer(dir.path(), "test", HashMap::new(), Vec::new());
        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/alice/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(Stage::Fetching),
            summary: "clone failed".to_string(),
            detail: "detail-a".to_string(),
            context: vec![],
        });
        reporter.report(ProgressEvent::PluginFailed {
            id: "github.com/bob/tmux-sensible",
            name: "tmux-sensible",
            stage: Some(Stage::Resolving),
            summary: "resolve failed".to_string(),
            detail: "detail-b".to_string(),
            context: vec![],
        });
        drop(reporter);

        let log_file = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .expect("log file should be created");
        let log = std::fs::read_to_string(log_file.path()).unwrap();

        assert!(log.contains("id=github.com/alice/tmux-sensible"), "log: {log}");
        assert!(log.contains("id=github.com/bob/tmux-sensible"), "log: {log}");
        assert!(log.contains("stage=fetching"), "log: {log}");
        assert!(log.contains("stage=resolving"), "log: {log}");
    }
}
