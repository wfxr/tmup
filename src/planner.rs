use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use crate::git;
use crate::lockfile::LockFile;
use crate::model::{Config, PluginSource, Tracking};
use crate::state::{Paths, build_command_hash};

/// Health of a declared plugin's target directory on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoHealth {
    /// Target directory does not exist.
    Missing,
    /// Valid git repo with readable HEAD.
    Healthy {
        /// The current HEAD commit hash.
        commit: String,
    },
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
    /// Plugin is present and matches the lock file.
    Installed,
    /// Plugin directory does not exist.
    Missing,
    /// Plugin is present but its commit differs from the lock file.
    Outdated,
    /// Plugin is pinned to a specific tag.
    PinnedTag,
    /// Plugin is pinned to a specific commit.
    PinnedCommit,
    /// Plugin is a local path reference.
    Local,
    /// Plugin directory exists but is not a valid git repository.
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

/// Result of a plugin's build hook, when configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// Build hook completed successfully.
    Ok,
    /// Build hook was run but failed.
    BuildFailed,
    /// No build hook is configured for this plugin.
    None,
}

impl fmt::Display for BuildStatus {
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
    /// Unique identifier for the plugin (remote ID or local path).
    pub id: String,
    /// Human-readable display name of the plugin.
    pub name: String,
    /// Source specifier string (URL or local path).
    pub source: String,
    /// Plugin kind: `"remote"` or `"local"`.
    pub kind: String,
    /// Current availability state of the plugin.
    pub state: PluginState,
    /// Build hook result ([`BuildStatus::None`] when no hook is configured).
    pub build_status: BuildStatus,
    /// HEAD commit currently checked out in the plugin directory.
    pub current_commit: Option<String>,
    /// Commit recorded in the lock file for this plugin.
    pub lock_commit: Option<String>,
}

/// Set of `(plugin_id, build_hash)` pairs that have uncleared failure markers.
/// Keyed without commit so that both fresh-install failures (no lock entry)
/// and update/restore failures (marker commit != lock commit) are detected.
pub type FailedBuilds = HashSet<(String, String)>;

/// Collect `(plugin_id, build_hash)` pairs from failure markers.
pub fn collect_failed_builds(markers: &[crate::state::FailureMarker]) -> FailedBuilds {
    markers.iter().map(|m| (m.plugin_id.clone(), m.build_hash.clone())).collect()
}

/// Compute plugin statuses from config, lock, repo health, and failure markers.
pub fn compute_statuses(
    config: &Config,
    lock: &LockFile,
    health_map: &HashMap<String, RepoHealth>,
    failed_builds: &FailedBuilds,
) -> Vec<PluginStatus> {
    let mut statuses = Vec::new();

    for spec in &config.plugins {
        match &spec.source {
            PluginSource::Remote { raw, id, .. } => {
                let health = health_map.get(id.as_str()).cloned().unwrap_or(RepoHealth::Missing);

                let lock_entry = lock.plugins.get(id.as_str());

                let (state, current_commit) = match &health {
                    RepoHealth::Missing => (PluginState::Missing, None),
                    RepoHealth::Broken => (PluginState::Broken, None),
                    RepoHealth::Healthy { commit } => {
                        let st = if let Some(locked) = lock_entry.map(|e| e.commit.as_str()) {
                            if commit != locked {
                                PluginState::Outdated
                            } else {
                                match &spec.tracking {
                                    Tracking::Tag(_) => PluginState::PinnedTag,
                                    Tracking::Commit(_) => PluginState::PinnedCommit,
                                    _ => PluginState::Installed,
                                }
                            }
                        } else {
                            match &spec.tracking {
                                Tracking::Tag(_) => PluginState::PinnedTag,
                                Tracking::Commit(_) => PluginState::PinnedCommit,
                                _ => PluginState::Installed,
                            }
                        };
                        (st, Some(commit.clone()))
                    }
                };

                let is_healthy = matches!(health, RepoHealth::Healthy { .. });

                // build-status: any uncleared failure marker for this plugin + build
                // means the last build failed. Success always clears markers.
                let build_status = if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    if failed_builds.contains(&(id.clone(), bh)) {
                        BuildStatus::BuildFailed
                    } else if is_healthy {
                        BuildStatus::Ok
                    } else {
                        BuildStatus::None
                    }
                } else {
                    BuildStatus::None
                };

                statuses.push(PluginStatus {
                    id: id.clone(),
                    name: spec.name.clone(),
                    source: raw.clone(),
                    kind: "remote".into(),
                    state,
                    build_status,
                    current_commit,
                    lock_commit: lock_entry.map(|e| e.commit.clone()),
                });
            }
            PluginSource::Local { path } => {
                let local_path = Path::new(path);
                let state = if !local_path.exists() {
                    PluginState::Missing
                } else if local_path.is_dir() {
                    PluginState::Local
                } else {
                    PluginState::Broken
                };

                statuses.push(PluginStatus {
                    id: path.clone(),
                    name: spec.name.clone(),
                    source: path.clone(),
                    kind: "local".into(),
                    state,
                    build_status: BuildStatus::None,
                    current_commit: None,
                    lock_commit: None,
                });
            }
        }
    }

    statuses
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

/// Build a health map for all declared remote plugins.
pub fn build_health_map(config: &Config, paths: &Paths) -> HashMap<String, RepoHealth> {
    config
        .plugins
        .iter()
        .filter_map(|spec| {
            let id = spec.remote_id()?;
            let health = inspect_plugin_dir(&paths.plugin_dir(id));
            Some((id.to_string(), health))
        })
        .collect()
}

fn scan_recursive_ids(root: &Path, current: &Path, ids: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            if path.join(".git").exists()
                && let Ok(rel) = path.strip_prefix(root)
            {
                ids.insert(rel.to_string_lossy().to_string());
            }
            continue;
        }
        if !file_type.is_dir() {
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
