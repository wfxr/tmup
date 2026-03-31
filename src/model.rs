/// Top-level configuration holding global options and the list of plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Global options that apply to all plugins.
    pub options: Options,
    /// Ordered list of plugin specifications.
    pub plugins: Vec<PluginSpec>,
}

/// Global options that control tmup behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Automatically install missing plugins on tmux startup when true.
    pub auto_install: bool,
    /// Maximum number of concurrent remote prepare jobs.
    pub concurrency: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { auto_install: true, concurrency: 16 }
    }
}

/// Describes where a plugin originates from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// Plugin hosted on a remote Git forge.
    Remote {
        /// Raw source string from config (e.g. "tmux-plugins/tmux-sensible")
        raw: String,
        /// Canonical id (e.g. "github.com/tmux-plugins/tmux-sensible")
        id: String,
        /// Resolved clone URL
        clone_url: String,
    },
    /// Plugin that lives on the local filesystem.
    Local {
        /// Absolute or home-relative path to the plugin directory.
        path: String,
    },
}

/// Specifies which Git ref a plugin should track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracking {
    /// Follow the repository's default branch.
    DefaultBranch,
    /// Follow a named branch.
    Branch(String),
    /// Pin to a specific tag.
    Tag(String),
    /// Pin to a specific commit hash.
    Commit(String),
}

/// Full specification for a single plugin entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    /// Origin of the plugin (remote URL or local path).
    pub source: PluginSource,
    /// Short display name derived from the source.
    pub name: String,
    /// Prefix used when setting tmux options for this plugin.
    pub opt_prefix: String,
    /// Which Git ref to track for updates.
    pub tracking: Tracking,
    /// Optional shell command to run after installing or updating.
    pub build: Option<String>,
    /// Extra key-value options passed to the plugin.
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
    /// Returns true if the plugin comes from a remote Git forge.
    pub fn is_remote(&self) -> bool {
        matches!(self.source, PluginSource::Remote { .. })
    }

    /// Returns true if the plugin resides on the local filesystem.
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
