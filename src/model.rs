/// Core data types for lazytmux.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub options: Options,
    pub plugins: Vec<PluginSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub concurrency: usize,
    pub auto_install: bool,
    pub auto_clean: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { concurrency: 8, auto_install: true, auto_clean: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    Remote {
        /// Raw source string from config (e.g. "tmux-plugins/tmux-sensible")
        raw: String,
        /// Canonical id (e.g. "github.com/tmux-plugins/tmux-sensible")
        id: String,
        /// Resolved clone URL
        clone_url: String,
    },
    Local {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracking {
    DefaultBranch,
    Branch(String),
    Tag(String),
    Commit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub source: PluginSource,
    pub name: String,
    pub opt_prefix: String,
    pub tracking: Tracking,
    pub build: Option<String>,
    pub opts: Vec<(String, String)>,
}

impl Config {
    /// Validate that a target id matches at least one remote plugin in config.
    pub fn validate_target_id(&self, target_id: Option<&str>) -> anyhow::Result<()> {
        if let Some(target) = target_id {
            let exists = self.plugins.iter().any(|p| p.remote_id() == Some(target));
            if !exists {
                anyhow::bail!("unknown plugin id: \"{target}\"");
            }
        }
        Ok(())
    }
}

impl PluginSpec {
    pub fn is_remote(&self) -> bool {
        matches!(self.source, PluginSource::Remote { .. })
    }

    pub fn is_local(&self) -> bool {
        matches!(self.source, PluginSource::Local { .. })
    }

    /// Returns the canonical remote id, or None for local plugins.
    pub fn remote_id(&self) -> Option<&str> {
        match &self.source {
            PluginSource::Remote { id, .. } => Some(id),
            PluginSource::Local { .. } => None,
        }
    }
}
