use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

use crate::model::{Config, PluginSource, PluginSpec, Tracking};

pub const LOCKFILE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockFile {
    pub version:            u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_fingerprint: Option<String>,
    pub plugins:            BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    pub source:      String,
    pub tracking:    TrackingRecord,
    pub commit:      String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackingRecord {
    #[serde(rename = "type")]
    pub kind:  String,
    pub value: String,
}

impl LockEntry {
    pub fn branch(source: &str, branch: &str, commit: &str) -> Self {
        Self {
            source:      source.into(),
            tracking:    TrackingRecord { kind: "branch".into(), value: branch.into() },
            commit:      commit.into(),
            config_hash: None,
        }
    }

    pub fn tag(source: &str, tag: &str, commit: &str) -> Self {
        Self {
            source:      source.into(),
            tracking:    TrackingRecord { kind: "tag".into(), value: tag.into() },
            commit:      commit.into(),
            config_hash: None,
        }
    }

    pub fn commit(source: &str, commit: &str) -> Self {
        Self {
            source:      source.into(),
            tracking:    TrackingRecord { kind: "commit".into(), value: commit.into() },
            commit:      commit.into(),
            config_hash: None,
        }
    }

    pub fn default_branch(source: &str, branch: &str, commit: &str) -> Self {
        Self {
            source:      source.into(),
            tracking:    TrackingRecord { kind: "default-branch".into(), value: branch.into() },
            commit:      commit.into(),
            config_hash: None,
        }
    }
}

impl LockFile {
    pub fn new() -> Self {
        Self {
            version:            LOCKFILE_VERSION,
            config_fingerprint: None,
            plugins:            BTreeMap::new(),
        }
    }
}

impl Default for LockFile {
    fn default() -> Self {
        Self::new()
    }
}

pub fn remote_plugin_config_hash(spec: &PluginSpec) -> Option<String> {
    let PluginSource::Remote { id, .. } = &spec.source else {
        return None;
    };

    let selector = match &spec.tracking {
        Tracking::DefaultBranch => HashTrackingSelector { kind: "default-branch", value: None },
        Tracking::Branch(branch) =>
            HashTrackingSelector { kind: "branch", value: Some(branch.as_str()) },
        Tracking::Tag(tag) => HashTrackingSelector { kind: "tag", value: Some(tag.as_str()) },
        Tracking::Commit(commit) =>
            HashTrackingSelector { kind: "commit", value: Some(commit.as_str()) },
    };

    Some(hash_json(&HashPluginInput {
        id,
        source: id,
        tracking: selector,
        build: spec.build.as_deref(),
    }))
}

pub fn config_fingerprint(config: &Config) -> String {
    let mut entries: Vec<_> = config
        .plugins
        .iter()
        .filter_map(|spec| {
            Some(HashFingerprintEntry {
                id:          spec.remote_id()?,
                config_hash: remote_plugin_config_hash(spec)?,
            })
        })
        .collect();
    entries.sort_unstable_by(|a, b| a.id.cmp(b.id));
    hash_json(&entries)
}

pub fn read_lockfile(path: &Path) -> Result<LockFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read lockfile: {}", path.display()))?;
    let lock: LockFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse lockfile: {}", path.display()))?;
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
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

#[derive(Serialize)]
struct HashPluginInput<'a> {
    id:       &'a str,
    source:   &'a str,
    tracking: HashTrackingSelector<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build:    Option<&'a str>,
}

#[derive(Serialize)]
struct HashTrackingSelector<'a> {
    kind:  &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a str>,
}

#[derive(Serialize)]
struct HashFingerprintEntry<'a> {
    id:          &'a str,
    config_hash: String,
}

fn hash_json<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("hash input serialization must succeed");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
