use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use owo_colors::OwoColorize;

use crate::model::Config;
use crate::state::Paths;
use crate::termui::{self, Accent};

const SUMMARY_MAX_LEN: usize = 80;
const ACTION_WIDTH: usize = 12;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Plugin-level processing stage.
#[derive(Debug, Clone, Copy)]
pub enum Stage {
    /// Clone the remote repository for the first time.
    Cloning,
    /// Fetch updates from the remote repository.
    Fetching,
    /// Resolve a branch, tag, or default-branch to a commit.
    Resolving,
    /// Check out the resolved commit into the working tree.
    CheckingOut,
    /// Run the plugin's build command and publish its files.
    Applying,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cloning => write!(f, "cloning"),
            Self::Fetching => write!(f, "fetching"),
            Self::Resolving => write!(f, "resolving"),
            Self::CheckingOut => write!(f, "checking out"),
            Self::Applying => write!(f, "publishing"),
        }
    }
}

/// Operation-level stage (non-plugin work visible to the user).
#[derive(Debug, Clone, Copy)]
pub enum OperationStage {
    /// Waiting to acquire the exclusive operation lock.
    WaitingForLock,
    /// Synchronising all remote plugins in parallel.
    Syncing,
    /// Writing resolved plugin contents to the filesystem.
    ApplyingWrites,
    /// Applying the load plan inside the running tmux session.
    LoadingTmux,
}

impl std::fmt::Display for OperationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaitingForLock => write!(f, "waiting"),
            Self::Syncing => write!(f, "syncing"),
            Self::ApplyingWrites => write!(f, "applying writes"),
            Self::LoadingTmux => write!(f, "loading tmux"),
        }
    }
}

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
        /// Optional stage-specific detail (URL, ref, build command, etc.).
        detail: Option<String>,
    },
    /// A plugin finished successfully with the given summary.
    PluginDone {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// Short description of what was done (e.g. `"updated abc -> def"`).
        summary: String,
    },
    /// A plugin was skipped with the given reason.
    PluginSkipped {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// Explanation of why the plugin was skipped.
        reason: String,
    },
    /// A plugin failed with a short summary and full diagnostic detail.
    PluginFailed {
        /// Stable remote identifier for the plugin.
        id: &'a str,
        /// Human-readable plugin name.
        name: &'a str,
        /// One-line failure summary suitable for terminal output.
        summary: String,
        /// Full diagnostic text written to the log file.
        detail: String,
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
    labels: HashMap<String, String>,
) -> Box<dyn ProgressReporter> {
    Box::new(StreamReporter::new(&paths.logs_root, command, labels))
}

/// Build stable display labels before progress output begins.
pub fn build_display_labels(config: &Config, target_id: Option<&str>) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
    let remote_plugins: Vec<_> = config
        .plugins
        .iter()
        .filter_map(|plugin| {
            let id = plugin.remote_id()?;
            target_id.is_none_or(|target| target == id).then_some((id, plugin.name.as_str()))
        })
        .collect();

    for (id, name) in &remote_plugins {
        by_name.entry(name).or_default().push(id);
    }

    for (id, name) in remote_plugins {
        let colliding_ids = &by_name[name];
        let label = if colliding_ids.len() == 1 {
            name.to_string()
        } else {
            let short = short_remote_id(id);
            let short_is_unique =
                colliding_ids.iter().filter(|other| short_remote_id(other) == short).count() == 1;
            if short_is_unique { short.to_string() } else { id.to_string() }
        };
        labels.insert(id.to_string(), label);
    }

    labels
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

/// Generate a log filename for the current operation.
fn log_filename(command: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pid = std::process::id();
    format!("{ts}-{pid}-{command}.log")
}

fn short_remote_id(id: &str) -> &str {
    id.split_once('/').map(|(_, tail)| tail).unwrap_or(id)
}

#[cfg(test)]
fn format_progress_line(action: &str, message: &str) -> String {
    termui::format_plain_labeled_line(action, ACTION_WIDTH, message)
}

fn split_done_summary(summary: &str) -> (&'static str, String) {
    let summary = sanitize(summary);
    for (prefix, action) in
        [("updated ", "Updated"), ("installed ", "Installed"), ("restored ", "Restored")]
    {
        if let Some(rest) = summary.strip_prefix(prefix) {
            return (action, rest.to_string());
        }
    }

    let action = if summary == "up-to-date" || summary == "already restored" {
        "Checked"
    } else if summary == "synced" {
        return ("Synced", String::new());
    } else if let Some(hash) = summary.strip_prefix("synced ") {
        return ("Synced", format!("commit@{hash}"));
    } else if summary == "lock reconciled" {
        "Reconciled"
    } else {
        "Done"
    };
    (action, summary)
}

/// Style reference tokens like `branch@master`, `commit@abc123` by
/// highlighting the value (after `@`) in magenta.
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

fn operation_message(stage: OperationStage) -> &'static str {
    match stage {
        OperationStage::WaitingForLock => "lock",
        OperationStage::Syncing => "remote plugins",
        OperationStage::ApplyingWrites => "plugin contents",
        OperationStage::LoadingTmux => "applying load plan",
    }
}

fn format_operation_failure(summary: &str) -> String {
    format!("operation {}", sanitize_summary(summary, SUMMARY_MAX_LEN))
}

fn title_case<T: std::fmt::Display>(value: T) -> String {
    let value = value.to_string();
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// DetailLog — shared lazy-create log file used by both reporters.
// ---------------------------------------------------------------------------

struct DetailLog {
    logs_root: PathBuf,
    log_path: PathBuf,
    file: Option<std::fs::File>,
}

impl DetailLog {
    fn new(logs_root: &Path, command: &str) -> Self {
        Self {
            logs_root: logs_root.to_path_buf(),
            log_path: logs_root.join(log_filename(command)),
            file: None,
        }
    }

    fn has_details(&self) -> bool {
        self.file.is_some()
    }

    fn write(&mut self, section: &str, summary: &str, detail: &str) {
        if self.file.is_none() {
            let _ = std::fs::create_dir_all(&self.logs_root);
            if let Ok(f) = std::fs::File::create(&self.log_path) {
                self.file = Some(f);
            }
        }
        if let Some(ref mut f) = self.file {
            let _ = writeln!(f, "== {section} ==");
            let _ = writeln!(f, "summary: {summary}");
            let _ = writeln!(f);
            let _ = writeln!(f, "{detail}");
            let _ = writeln!(f);
        }
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
enum LineKind {
    Header,
    Stage,
    Success,
    Warning,
    Failure,
    Muted,
}

struct RenderedLine {
    kind: LineKind,
    action: String,
    message: String,
}

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

struct StreamRenderer {
    labels: HashMap<String, String>,
}

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
                        self.stage_message(id, name, stage, detail.as_deref()),
                    )]
                }
            },
            ProgressEvent::PluginDone { id, name, summary } => {
                let (action, suffix) = split_done_summary(&summary);
                let label = self.label(id, name);
                let message = if suffix.is_empty() {
                    label.to_string()
                } else {
                    format!("{label} {}", style_ref_tokens(&suffix))
                };
                vec![RenderedLine::new(LineKind::Success, action, message)]
            }
            ProgressEvent::PluginSkipped { id, name, reason } => {
                vec![RenderedLine::new(
                    LineKind::Warning,
                    "Skipped",
                    format!(
                        "{} {}",
                        self.label(id, name),
                        sanitize_summary(&reason, SUMMARY_MAX_LEN)
                    ),
                )]
            }
            ProgressEvent::PluginFailed { id, name, summary, .. } => {
                vec![RenderedLine::new(
                    LineKind::Failure,
                    "Failed",
                    format!(
                        "{} {}",
                        self.label(id, name),
                        sanitize_summary(&summary, SUMMARY_MAX_LEN)
                    ),
                )]
            }
            ProgressEvent::OperationFailed { summary, .. } => {
                vec![RenderedLine::new(
                    LineKind::Failure,
                    "Failed",
                    format_operation_failure(&summary),
                )]
            }
            ProgressEvent::OperationEnd { command: "init" } => {
                vec![RenderedLine::new(LineKind::Success, "Finished", "lazytmux init")]
            }
            ProgressEvent::OperationEnd { .. } => Vec::new(),
        }
    }

    fn label<'a>(&'a self, id: &'a str, name: &'a str) -> &'a str {
        self.labels.get(id).map(String::as_str).unwrap_or(name)
    }

    fn stage_message(&self, id: &str, name: &str, stage: Stage, detail: Option<&str>) -> String {
        let label = self.label(id, name);
        match (stage, detail) {
            (Stage::Cloning | Stage::Fetching, Some(url)) => {
                format!("{label} {}", url.blue())
            }
            (Stage::Resolving, Some(detail)) => {
                format!("{label} {}", style_ref_tokens(detail))
            }
            (Stage::Applying, Some(build_cmd)) => {
                format!("{label} {}", sanitize_summary(build_cmd, SUMMARY_MAX_LEN))
            }
            _ => label.to_string(),
        }
    }
}

impl From<LineKind> for Accent {
    fn from(value: LineKind) -> Self {
        match value {
            LineKind::Header => Accent::Bold,
            LineKind::Stage => Accent::Info,
            LineKind::Success => Accent::Success,
            LineKind::Warning => Accent::Warning,
            LineKind::Failure => Accent::Error,
            LineKind::Muted => Accent::Muted,
        }
    }
}

// ---------------------------------------------------------------------------
// Stream reporter
// ---------------------------------------------------------------------------

struct StreamReporter<W: Write + Send> {
    state: Mutex<StreamReporterInner<W>>,
}

struct StreamReporterInner<W: Write> {
    writer: W,
    renderer: StreamRenderer,
    log: DetailLog,
    finished: bool,
}

impl StreamReporter<anstream::AutoStream<std::io::Stderr>> {
    fn new(logs_root: &Path, command: &str, labels: HashMap<String, String>) -> Self {
        Self::new_with_writer(logs_root, command, labels, anstream::stderr())
    }
}

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
                inner.log.log_path.display().to_string(),
            );
            let _ = writeln!(inner.writer, "{}", line.styled());
        }
    }
}

impl<W: Write + Send> ProgressReporter for StreamReporter<W> {
    fn report(&self, event: ProgressEvent<'_>) {
        let mut inner = self.state.lock().unwrap();
        let should_finish = matches!(event, ProgressEvent::OperationEnd { .. });
        match &event {
            ProgressEvent::PluginFailed { name, summary, detail, .. } => {
                let summary = sanitize_summary(summary, SUMMARY_MAX_LEN);
                inner.log.write(&format!("plugin: {name}"), &summary, detail);
            }
            ProgressEvent::OperationFailed { summary, detail } => {
                let summary = sanitize_summary(summary, SUMMARY_MAX_LEN);
                inner.log.write("operation", &summary, detail);
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
            renderer.render(ProgressEvent::PluginDone {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                summary: "updated abc1234 -> def5678".to_string(),
            }),
            vec!["     Updated tmux-sensible abc1234 -> def5678".to_string()]
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
                detail: Some("make build".to_string()),
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
            renderer.render(ProgressEvent::PluginDone {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                summary: "synced 8c1eeec".to_string(),
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
                detail: Some("https://github.com/tmux-plugins/tmux-sensible".to_string()),
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
                detail: Some("tag@v1.0 -> commit@8c1eeec".to_string()),
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
                detail: Some("branch@main -> commit@8c1eeec".to_string()),
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
                detail: Some("default-branch -> branch@main -> commit@8c1eeec".to_string()),
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
            renderer.render(ProgressEvent::PluginSkipped {
                id: "github.com/tmux-plugins/tmux-sensible",
                name: "tmux-sensible",
                reason: "pinned to tag v1.0.0".to_string(),
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
                summary: "git clone --bare failed".to_string(),
                detail: String::new(),
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
    fn split_done_summary_extracts_action_and_suffix() {
        let (action, suffix) = split_done_summary("updated abc1234 -> def5678");
        assert_eq!(action, "Updated");
        assert_eq!(suffix, "abc1234 -> def5678");
    }

    #[test]
    fn split_done_summary_preserves_non_prefixed_summaries() {
        let (action, suffix) = split_done_summary("lock reconciled");
        assert_eq!(action, "Reconciled");
        assert_eq!(suffix, "lock reconciled");
    }

    #[test]
    fn split_done_summary_omits_duplicate_synced_suffix() {
        let (action, suffix) = split_done_summary("synced");
        assert_eq!(action, "Synced");
        assert!(suffix.is_empty());
    }

    #[test]
    fn split_done_summary_formats_synced_hash_as_suffix() {
        let (action, suffix) = split_done_summary("synced 8c1eeec");
        assert_eq!(action, "Synced");
        assert_eq!(suffix, "commit@8c1eeec");
    }
}
