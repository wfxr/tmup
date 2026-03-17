use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LockFile {
    pub version: u32,
    pub plugins: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    pub source:   String,
    pub tracking: TrackingRecord,
    pub commit:   String,
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
            source:   source.into(),
            tracking: TrackingRecord { kind: "branch".into(), value: branch.into() },
            commit:   commit.into(),
        }
    }

    pub fn tag(source: &str, tag: &str, commit: &str) -> Self {
        Self {
            source:   source.into(),
            tracking: TrackingRecord { kind: "tag".into(), value: tag.into() },
            commit:   commit.into(),
        }
    }

    pub fn commit(source: &str, commit: &str) -> Self {
        Self {
            source:   source.into(),
            tracking: TrackingRecord { kind: "commit".into(), value: commit.into() },
            commit:   commit.into(),
        }
    }
}

impl LockFile {
    pub fn new() -> Self {
        Self { version: 1, plugins: BTreeMap::new() }
    }
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
    let json = serde_json::to_string_pretty(lock).context("failed to serialize lockfile")?;

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
