use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Config, PluginSource, PluginSpec, Tracking};

/// Current lockfile format version understood by this build.
pub const LOCKFILE_VERSION: u32 = 2;

/// Serialized state of all locked plugins, written to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockFile {
    /// Format version used to detect incompatible lockfile changes.
    pub version: u32,
    /// SHA-256 fingerprint of the full plugin configuration, used to detect config drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_fingerprint: Option<String>,
    /// Map from plugin id to its locked entry.
    pub plugins: BTreeMap<String, LockEntry>,
}

/// Locked state for a single plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    /// How the plugin is tracked (branch, tag, or pinned commit).
    pub tracking: TrackingRecord,
    /// Exact commit SHA that is currently checked out.
    pub commit: String,
    /// Hash of the plugin's configuration at lock time, used to detect config drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
}

/// Serialized form of the tracking strategy stored inside a [`LockEntry`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackingRecord {
    /// Discriminator string (`"branch"`, `"tag"`, `"commit"`, or `"default-branch"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// The branch name, tag name, or commit SHA being tracked.
    pub value: String,
}

impl LockEntry {
    /// Creates a [`LockEntry`] that tracks a named branch.
    pub fn branch(branch: &str, commit: &str) -> Self {
        Self {
            tracking: TrackingRecord { kind: "branch".into(), value: branch.into() },
            commit: commit.into(),
            config_hash: None,
        }
    }

    /// Creates a [`LockEntry`] that tracks a named tag.
    pub fn tag(tag: &str, commit: &str) -> Self {
        Self {
            tracking: TrackingRecord { kind: "tag".into(), value: tag.into() },
            commit: commit.into(),
            config_hash: None,
        }
    }

    /// Creates a [`LockEntry`] pinned to a specific commit SHA.
    pub fn commit(commit: &str) -> Self {
        Self {
            tracking: TrackingRecord { kind: "commit".into(), value: commit.into() },
            commit: commit.into(),
            config_hash: None,
        }
    }

    /// Creates a [`LockEntry`] that tracks the repository's default branch.
    pub fn default_branch(branch: &str, commit: &str) -> Self {
        Self {
            tracking: TrackingRecord { kind: "default-branch".into(), value: branch.into() },
            commit: commit.into(),
            config_hash: None,
        }
    }
}

impl LockFile {
    /// Creates an empty lockfile at the current format version.
    pub fn new() -> Self {
        Self { version: LOCKFILE_VERSION, config_fingerprint: None, plugins: BTreeMap::new() }
    }
}

impl Default for LockFile {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns a SHA-256 hash of the configuration fields that affect a remote plugin's lock state, or `None` for local plugins.
pub fn remote_plugin_config_hash(spec: &PluginSpec) -> Option<String> {
    let PluginSource::Remote { id, .. } = &spec.source else {
        return None;
    };

    let selector = match &spec.tracking {
        Tracking::DefaultBranch => HashTrackingSelector { kind: "default-branch", value: None },
        Tracking::Branch(branch) => {
            HashTrackingSelector { kind: "branch", value: Some(branch.as_str()) }
        }
        Tracking::Tag(tag) => HashTrackingSelector { kind: "tag", value: Some(tag.as_str()) },
        Tracking::Commit(commit) => {
            HashTrackingSelector { kind: "commit", value: Some(commit.as_str()) }
        }
    };

    Some(hash_json(&HashPluginInput {
        id,
        source: id,
        tracking: selector,
        build: spec.build.as_deref(),
    }))
}

/// Computes a single SHA-256 fingerprint that covers all remote plugins in `config`.
pub fn config_fingerprint(config: &Config) -> String {
    let mut entries: Vec<_> = config
        .plugins
        .iter()
        .filter_map(|spec| {
            Some(HashFingerprintEntry {
                id: spec.remote_id()?,
                config_hash: remote_plugin_config_hash(spec)?,
            })
        })
        .collect();
    entries.sort_unstable_by(|a, b| a.id.cmp(b.id));
    hash_json(&entries)
}

/// Reads and deserializes a lockfile from `path`, returning an error if the version is unsupported.
pub fn read_lockfile(path: &Path) -> Result<LockFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read lockfile: {}", path.display()))?;
    let lock: LockFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse lockfile: {}", path.display()))?;
    anyhow::ensure!(
        lock.version == LOCKFILE_VERSION,
        "unsupported lockfile version: {} (expected {})",
        lock.version,
        LOCKFILE_VERSION
    );
    Ok(lock)
}

/// Atomic write: write to .tmp, fsync, then rename.
pub fn write_lockfile_atomic(path: &Path, lock: &LockFile) -> Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    let mut normalized = lock.clone();
    normalized.version = LOCKFILE_VERSION;
    let json = serde_json::to_string_pretty(&normalized).context("failed to serialize lockfile")?;

    if let Some(parent) = tmp_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }

    let mut file = fs::File::create(&tmp_path)
        .with_context(|| format!("failed to create {}", tmp_path.display()))?;
    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);

    fs::rename(&tmp_path, path).with_context(|| {
        format!("failed to rename {} -> {}", tmp_path.display(), path.display())
    })?;

    Ok(())
}

#[derive(Serialize)]
struct HashPluginInput<'a> {
    id: &'a str,
    source: &'a str,
    tracking: HashTrackingSelector<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<&'a str>,
}

#[derive(Serialize)]
struct HashTrackingSelector<'a> {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
}

#[derive(Serialize)]
struct HashFingerprintEntry<'a> {
    id: &'a str,
    config_hash: String,
}

fn hash_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("hash input serialization must succeed");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    base16ct::lower::encode_string(&hasher.finalize())
}
