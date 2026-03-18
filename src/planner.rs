use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
};

use crate::{
    git,
    lockfile::LockFile,
    model::{Config, PluginSource, Tracking},
    state::build_command_hash,
};

/// Health of a declared plugin's target directory on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoHealth {
    /// Target directory does not exist.
    Missing,
    /// Valid git repo with readable HEAD.
    Healthy { commit: String },
    /// Directory exists but is not a valid git repo or HEAD is unreadable.
    Broken,
}

/// Inspect a declared plugin's target directory.
///
/// Unlike `scan_managed_plugin_ids` which discovers unknown directories,
/// this checks a known path — so it correctly detects "directory exists
/// but is not a git repo" as `Broken` rather than invisible.
pub fn inspect_plugin_dir(path: &Path) -> RepoHealth {
    if !path.exists() {
        return RepoHealth::Missing;
    }
    if path.join(".git").exists() {
        match git::head_commit_sync(path) {
            Ok(commit) => RepoHealth::Healthy { commit },
            Err(_) => RepoHealth::Broken,
        }
    } else {
        RepoHealth::Broken
    }
}

/// Current availability state of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Installed,
    Missing,
    Outdated,
    PinnedTag,
    PinnedCommit,
    Local,
    Broken,
}

impl fmt::Display for PluginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed => write!(f, "installed"),
            Self::Missing => write!(f, "missing"),
            Self::Outdated => write!(f, "outdated"),
            Self::PinnedTag => write!(f, "pinned-tag"),
            Self::PinnedCommit => write!(f, "pinned-commit"),
            Self::Local => write!(f, "local"),
            Self::Broken => write!(f, "broken"),
        }
    }
}

/// Result of the most recent build/operation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastResult {
    Ok,
    BuildFailed,
    None,
}

impl fmt::Display for LastResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "ok"),
            Self::BuildFailed => write!(f, "build-failed"),
            Self::None => write!(f, "none"),
        }
    }
}

/// A row of plugin status for list display.
#[derive(Debug, Clone)]
pub struct PluginStatus {
    pub id:             String,
    pub name:           String,
    pub source:         String,
    pub kind:           String,
    pub state:          PluginState,
    pub last_result:    LastResult,
    pub current_commit: Option<String>,
    pub lock_commit:    Option<String>,
}

/// Plan for write operations during init.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritePlan {
    /// Remote plugins that need to be installed (missing from disk).
    pub to_install: Vec<String>,
    /// Remote plugins that need to be restored (installed but at wrong commit).
    pub to_restore: Vec<String>,
    /// Remote plugins that should be cleaned (undeclared).
    pub to_clean:   Vec<String>,
}

/// Set of `(plugin_id, build_hash)` pairs that have uncleared failure markers.
/// Keyed without commit so that both fresh-install failures (no lock entry)
/// and update/restore failures (marker commit != lock commit) are detected.
pub type FailedBuilds = HashSet<(String, String)>;

/// Collect `(plugin_id, build_hash)` pairs from failure markers.
pub fn collect_failed_builds(markers: &[crate::state::FailureMarker]) -> FailedBuilds {
    markers
        .iter()
        .map(|m| (m.plugin_id.clone(), m.build_hash.clone()))
        .collect()
}

/// Compute plugin statuses from config, lock, installed state, and failure markers.
pub fn compute_statuses(
    config: &Config,
    lock: &LockFile,
    installed_plugins: &HashMap<String, Option<String>>,
    failed_builds: &FailedBuilds,
) -> Vec<PluginStatus> {
    let mut statuses = Vec::new();

    for spec in &config.plugins {
        match &spec.source {
            PluginSource::Remote { raw, id, .. } => {
                let installed_entry = installed_plugins.get(id.as_str());
                let is_installed = installed_entry.is_some();
                let current_commit = installed_entry.and_then(|c| c.clone());

                let lock_entry = lock.plugins.get(id.as_str());
                let lock_commit_str = lock_entry.map(|e| e.commit.as_str());

                let state = if !is_installed {
                    PluginState::Missing
                } else {
                    match &spec.tracking {
                        Tracking::Tag(_) => PluginState::PinnedTag,
                        Tracking::Commit(_) => PluginState::PinnedCommit,
                        _ => {
                            // Check if installed commit has drifted from lock
                            if let (Some(cur), Some(locked)) =
                                (current_commit.as_deref(), lock_commit_str)
                            {
                                if cur != locked {
                                    PluginState::Outdated
                                } else {
                                    PluginState::Installed
                                }
                            } else {
                                PluginState::Installed
                            }
                        }
                    }
                };

                // last-result: any uncleared failure marker for this plugin + build
                // means the last operation failed. Success always clears markers.
                let last_result = if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    if failed_builds.contains(&(id.clone(), bh)) {
                        LastResult::BuildFailed
                    } else if is_installed {
                        LastResult::Ok
                    } else {
                        LastResult::None
                    }
                } else if is_installed {
                    LastResult::Ok
                } else {
                    LastResult::None
                };

                statuses.push(PluginStatus {
                    id: id.clone(),
                    name: spec.name.clone(),
                    source: raw.clone(),
                    kind: "remote".into(),
                    state,
                    last_result,
                    current_commit,
                    lock_commit: lock_entry.map(|e| e.commit.clone()),
                });
            }
            PluginSource::Local { path } => {
                statuses.push(PluginStatus {
                    id:             path.clone(),
                    name:           spec.name.clone(),
                    source:         path.clone(),
                    kind:           "local".into(),
                    state:          PluginState::Local,
                    last_result:    LastResult::Ok,
                    current_commit: None,
                    lock_commit:    None,
                });
            }
        }
    }

    statuses
}

/// Plan the init decision based on config, lock, and filesystem state.
///
/// Returns `Some(plan)` when writes are needed, `None` when everything is
/// aligned and plugins can be loaded directly.
pub fn plan_init(
    config: &Config,
    lock: &LockFile,
    installed_plugins: &HashMap<String, Option<String>>,
) -> Option<WritePlan> {
    let declared_ids: HashSet<&str> = config
        .plugins
        .iter()
        .filter_map(|p| p.remote_id())
        .collect();

    // Check what needs installing (missing from disk)
    let to_install: Vec<String> = if config.options.auto_install {
        declared_ids
            .iter()
            .filter(|id| !installed_plugins.contains_key(**id))
            .map(|id| id.to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Check what needs restoring (installed but at wrong commit vs lock)
    let to_restore: Vec<String> = declared_ids
        .iter()
        .filter(|id| {
            if let Some(installed_commit) = installed_plugins.get(**id) {
                if let Some(lock_entry) = lock.plugins.get(**id) {
                    // Installed commit differs from locked commit
                    match installed_commit {
                        Some(c) => c != &lock_entry.commit,
                        None => true, // couldn't read HEAD, treat as needing restore
                    }
                } else {
                    false // no lock entry, nothing to restore to
                }
            } else {
                false // not installed, handled by to_install
            }
        })
        .map(|id| id.to_string())
        .collect();

    // Check what needs cleaning
    let to_clean: Vec<String> = if config.options.auto_clean {
        installed_plugins
            .keys()
            .filter(|id| !declared_ids.contains(id.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let needs_write = !to_install.is_empty() || !to_restore.is_empty() || !to_clean.is_empty();

    if needs_write {
        Some(WritePlan { to_install, to_restore, to_clean })
    } else {
        None
    }
}

/// Discover managed plugin IDs on disk (directories containing `.git`).
///
/// Used by `clean` to find undeclared plugins. For declared plugins, use
/// `inspect_plugin_dir` instead — it correctly detects broken directories.
pub fn scan_managed_plugin_ids(plugin_root: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    scan_recursive_ids(plugin_root, plugin_root, &mut ids);
    ids
}

fn scan_recursive_ids(root: &Path, current: &Path, ids: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(".git").exists() {
            if let Ok(rel) = path.strip_prefix(root) {
                ids.insert(rel.to_string_lossy().to_string());
            }
        } else {
            scan_recursive_ids(root, &path, ids);
        }
    }
}

/// Scan the plugin root for installed remote plugin ids and their HEAD commits.
#[deprecated(
    note = "use inspect_plugin_dir for declared plugins, scan_managed_plugin_ids for clean"
)]
pub fn scan_installed_plugins(plugin_root: &Path) -> HashMap<String, Option<String>> {
    let mut installed = HashMap::new();
    scan_recursive(plugin_root, plugin_root, &mut installed);
    installed
}

fn scan_recursive(root: &Path, current: &Path, installed: &mut HashMap<String, Option<String>>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Check if this looks like a plugin directory (has *.tmux files or .git)
        if path.join(".git").exists() {
            if let Ok(rel) = path.strip_prefix(root) {
                let id = rel.to_string_lossy().to_string();
                let commit = git::head_commit_sync(&path).ok();
                installed.insert(id, commit);
            }
        } else {
            // Recurse into host/owner directories
            scan_recursive(root, &path, installed);
        }
    }
}
