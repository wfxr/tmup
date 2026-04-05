use std::io::Write;
use std::path::{Path, PathBuf};

use crate::progress::PluginStage;

/// Lazily-created failure detail log shared by reporter implementations.
pub(crate) struct DetailLog {
    logs_root: PathBuf,
    log_path: PathBuf,
    file: Option<std::fs::File>,
}

impl DetailLog {
    /// Create a detail log for one command execution.
    pub(crate) fn new(logs_root: &Path, command: &str) -> Self {
        Self {
            logs_root: logs_root.to_path_buf(),
            log_path: logs_root.join(log_filename(command)),
            file: None,
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

    fn write(&mut self, section: &str, summary: &str, detail: &str, context: &[(&str, &str)]) {
        if self.file.is_none() {
            let _ = std::fs::create_dir_all(&self.logs_root);
            if let Ok(f) = std::fs::File::create(&self.log_path) {
                self.file = Some(f);
            }
        }
        if let Some(ref mut f) = self.file {
            let _ = writeln!(f, "== {section} ==");
            let _ = writeln!(f, "summary: {summary}");
            for (key, value) in context {
                let _ = writeln!(f, "{key}: {value}");
            }
            let _ = writeln!(f);
            let _ = writeln!(f, "{detail}");
            let _ = writeln!(f);
        }
    }
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
}
