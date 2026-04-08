use std::io::Write;
use std::path::{Path, PathBuf};

use crate::progress::PluginStage;

/// Shared progress-output I/O diagnostic action taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProgressIoAction {
    HideCursor,
    RowUpdate,
    ShowCursor,
    Flush,
}

impl ProgressIoAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::HideCursor => "hide_cursor",
            Self::RowUpdate => "row_update",
            Self::ShowCursor => "show_cursor",
            Self::Flush => "flush",
        }
    }
}

/// Lazily-created failure detail log shared by reporter implementations.
pub(crate) struct DetailLog {
    logs_root: PathBuf,
    log_path: PathBuf,
    file: Option<std::fs::File>,
    pending_warning: Option<String>,
    warning_emitted: bool,
}

impl DetailLog {
    /// Create a detail log for one command execution.
    pub(crate) fn new(logs_root: &Path, command: &str) -> Self {
        Self {
            logs_root: logs_root.to_path_buf(),
            log_path: logs_root.join(log_filename(command)),
            file: None,
            pending_warning: None,
            warning_emitted: false,
        }
    }

    /// Return `true` when at least one detail section has been recorded.
    pub(crate) fn has_details(&self) -> bool {
        self.file.is_some()
    }

    /// Return the detail log path.
    pub(crate) fn path(&self) -> &Path {
        &self.log_path
    }

    /// Take the next user-visible warning produced while opening the detail log.
    pub(crate) fn take_warning(&mut self) -> Option<String> {
        self.pending_warning.take()
    }

    /// Record one plugin failure section.
    pub(crate) fn record_plugin_failure(
        &mut self,
        id: &str,
        name: &str,
        stage: Option<PluginStage>,
        summary: &str,
        detail: &str,
        context: &[(&str, &str)],
    ) {
        let mut section = format!("plugin id={id} name={name}");
        if let Some(stage) = stage {
            use std::fmt::Write;
            let _ = write!(section, " stage={stage}");
        }
        self.write(&section, summary, detail, context);
    }

    /// Record one operation-level failure section.
    pub(crate) fn record_operation_failure(&mut self, summary: &str, detail: &str) {
        self.write("operation", summary, detail, &[]);
    }

    /// Record one internal progress-output I/O diagnostic section.
    pub(crate) fn record_progress_io_diagnostic(&mut self, action: ProgressIoAction, detail: &str) {
        let section = format!("progress io action={}", action.as_str());
        self.write(&section, "terminal output write failure", detail, &[]);
    }

    fn write(&mut self, section: &str, summary: &str, detail: &str, context: &[(&str, &str)]) {
        if self.file.is_none() {
            if let Err(err) = std::fs::create_dir_all(&self.logs_root) {
                self.record_open_warning(&err);
            } else if let Err(err) =
                std::fs::File::create(&self.log_path).map(|file| self.file = Some(file))
            {
                self.record_open_warning(&err);
            }
        }
        if let Some(ref mut f) = self.file
            && let Err(err) = write_section(f, section, summary, detail, context)
        {
            self.record_append_warning(&err);
        }
    }

    fn record_open_warning(&mut self, err: &std::io::Error) {
        if self.warning_emitted {
            return;
        }
        self.warning_emitted = true;
        self.pending_warning =
            Some(format!("failed to write detail log {}: {}", self.log_path.display(), err));
    }

    fn record_append_warning(&mut self, err: &std::io::Error) {
        if self.warning_emitted {
            return;
        }
        self.warning_emitted = true;
        self.pending_warning =
            Some(format!("failed to append detail log {}: {}", self.log_path.display(), err));
    }
}

fn write_section(
    writer: &mut std::fs::File,
    section: &str,
    summary: &str,
    detail: &str,
    context: &[(&str, &str)],
) -> std::io::Result<()> {
    writeln!(writer, "== {section} ==")?;
    writeln!(writer, "summary: {summary}")?;
    for (key, value) in context {
        writeln!(writer, "{key}: {value}")?;
    }
    writeln!(writer)?;
    writeln!(writer, "{detail}")?;
    writeln!(writer)?;
    Ok(())
}

fn log_filename(command: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pid = std::process::id();
    format!("{ts}-{pid}-{command}.log")
}

#[cfg(test)]
mod tests {
    use super::DetailLog;
    use crate::progress::PluginStage;

    #[test]
    fn detail_log_includes_canonical_plugin_identity_and_stage() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = DetailLog::new(dir.path(), "test");

        log.record_plugin_failure(
            "github.com/tmux-plugins/tmux-sensible",
            "tmux-sensible",
            Some(PluginStage::Fetching),
            "git fetch origin failed",
            "full error output here",
            &[
                ("clone_url", "https://github.com/tmux-plugins/tmux-sensible.git"),
                ("tracking", "default-branch"),
            ],
        );

        assert!(log.has_details());
        let content = std::fs::read_to_string(log.path()).unwrap();

        assert!(content.contains("id=github.com/tmux-plugins/tmux-sensible"), "log: {content}");
        assert!(content.contains("name=tmux-sensible"), "log: {content}");
        assert!(content.contains("stage=fetching"), "log: {content}");
        assert!(content.contains("summary: git fetch origin failed"), "log: {content}");
        assert!(
            content.contains("clone_url: https://github.com/tmux-plugins/tmux-sensible.git"),
            "log: {content}"
        );
        assert!(content.contains("tracking: default-branch"), "log: {content}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detail_log_surfaces_warning_when_write_fails_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = DetailLog::new(dir.path(), "test");
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        log.file = Some(full);

        log.record_operation_failure("summary", "detail");

        let warning = log.take_warning();
        assert!(
            warning.as_deref().is_some_and(|text| text.contains("failed to append detail log")),
            "warning: {warning:?}"
        );
    }
}
