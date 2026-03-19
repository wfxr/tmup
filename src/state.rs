use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use etcetera::BaseStrategy;
use sha2::{Digest, Sha256};

/// All filesystem paths used by lazytmux.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Root for plugin checkouts: {data}/plugins/
    pub plugin_root: PathBuf,
    /// Staging area for in-progress installs: {data}/.staging/
    pub staging_root: PathBuf,
    /// Backup area for replace rollback: {data}/.backup/
    pub backup_root: PathBuf,
    /// Lock file for serializing write operations: {state}/operations.lock
    pub lock_path: PathBuf,
    /// Build failure markers: {state}/failures/
    pub failures_root: PathBuf,
    /// Active config file path
    pub config_path: PathBuf,
    /// Active lock file (usually next to the active config file)
    pub lockfile_path: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let base_dirs = etcetera::base_strategy::choose_base_strategy()
            .context("failed to determine XDG base directories")?;
        let data_dir = base_dirs.data_dir().join("lazytmux");
        let state_dir = base_dirs
            .state_dir()
            .unwrap_or_else(|| base_dirs.home_dir().join(".local/state"))
            .join("lazytmux");
        let config_dir = base_dirs.config_dir().join("tmux");

        Ok(Self {
            plugin_root: data_dir.join("plugins"),
            staging_root: data_dir.join(".staging"),
            backup_root: data_dir.join(".backup"),
            lock_path: state_dir.join("operations.lock"),
            failures_root: state_dir.join("failures"),
            config_path: config_dir.join("lazy.kdl"),
            lockfile_path: config_dir.join("lazylock.json"),
        })
    }

    /// Create a Paths for testing with explicit roots.
    pub fn for_test(data_root: impl Into<PathBuf>, state_root: impl Into<PathBuf>) -> Self {
        let data = data_root.into();
        let state = state_root.into();
        Self {
            plugin_root: data.join("plugins"),
            staging_root: data.join(".staging"),
            backup_root: data.join(".backup"),
            lock_path: state.join("operations.lock"),
            failures_root: state.join("failures"),
            config_path: state.join("lazy.kdl"),
            lockfile_path: state.join("lazylock.json"),
        }
    }

    pub fn set_config_path(&mut self, config_path: PathBuf) -> Result<()> {
        let config_dir = config_path.parent().with_context(|| {
            format!("config path has no parent directory: {}", config_path.display())
        })?;
        self.lockfile_path = config_dir.join("lazylock.json");
        self.config_path = config_path;
        Ok(())
    }

    /// Ensure all required directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.plugin_root)?;
        fs::create_dir_all(&self.staging_root)?;
        fs::create_dir_all(&self.backup_root)?;
        fs::create_dir_all(&self.failures_root)?;
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Get the install directory for a remote plugin by id.
    pub fn plugin_dir(&self, id: &str) -> PathBuf {
        self.plugin_root.join(checked_plugin_id(id))
    }

    /// Create a staging directory for a plugin operation.
    pub fn staging_dir(&self, id: &str) -> PathBuf {
        let id = checked_plugin_id(id);
        let hash = &build_command_hash(id)[..12];
        let pid = std::process::id();
        self.staging_root.join(format!("{hash}-{pid}"))
    }

    /// Create a backup directory path for a plugin.
    pub fn backup_dir(&self, id: &str) -> PathBuf {
        let id = checked_plugin_id(id);
        let hash = &build_command_hash(id)[..12];
        let pid = std::process::id();
        self.backup_root.join(format!("{hash}-{pid}"))
    }
}

pub(crate) fn validate_plugin_id(id: &str) -> Result<()> {
    ensure!(!id.is_empty(), "unsafe plugin id: empty");
    for segment in id.split('/') {
        ensure!(
            !segment.is_empty() && segment != "." && segment != ".." && !segment.contains('\\'),
            "unsafe plugin id segment: {segment:?}"
        );
    }
    Ok(())
}

fn checked_plugin_id(id: &str) -> &str {
    validate_plugin_id(id).expect("plugin ids must be validated before path construction");
    id
}

fn map_try_write_result<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(guard) => Ok(Some(guard)),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(err) => Err(err),
    }
}

/// SHA-256 hash of a string, returned as hex.
pub fn build_command_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Key for identifying a known build failure, used to suppress auto-retry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FailureKey {
    pub plugin_id: String,
    pub commit: String,
    pub build_hash: String,
}

impl FailureKey {
    pub fn new(plugin_id: &str, commit: &str, build_hash: &str) -> Self {
        Self { plugin_id: plugin_id.into(), commit: commit.into(), build_hash: build_hash.into() }
    }

    /// Derive the filename for persisting this failure marker.
    pub fn filename(&self) -> String {
        let combined = format!("{}:{}:{}", self.plugin_id, self.commit, self.build_hash);
        let hash = build_command_hash(&combined);
        format!("{}.json", &hash[..16])
    }
}

/// A persisted record of a build failure.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FailureMarker {
    pub plugin_id: String,
    pub commit: String,
    pub build_hash: String,
    pub build_command: String,
    pub failed_at: String,
    pub stderr_summary: String,
}

impl FailureMarker {
    pub fn key(&self) -> FailureKey {
        FailureKey::new(&self.plugin_id, &self.commit, &self.build_hash)
    }
}

/// Write a failure marker to disk.
pub fn write_failure_marker(failures_root: &Path, marker: &FailureMarker) -> Result<()> {
    fs::create_dir_all(failures_root)?;
    let key = marker.key();
    let path = failures_root.join(key.filename());
    let json = serde_json::to_string_pretty(marker)?;
    fs::write(&path, json)?;
    Ok(())
}

/// Read all failure markers from disk.
pub fn read_failure_markers(failures_root: &Path) -> Result<Vec<FailureMarker>> {
    let mut markers = Vec::new();
    if !failures_root.exists() {
        return Ok(markers);
    }
    for entry in fs::read_dir(failures_root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    if let Ok(marker) = serde_json::from_str::<FailureMarker>(&content) {
                        markers.push(marker);
                    }
                }
                Err(_) => continue,
            }
        }
    }
    Ok(markers)
}

/// Remove failure markers matching a specific plugin id.
pub fn clear_failure_markers(failures_root: &Path, plugin_id: &str) -> Result<()> {
    if !failures_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(failures_root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json")
            && let Ok(content) = fs::read_to_string(&path)
            && let Ok(marker) = serde_json::from_str::<FailureMarker>(&content)
            && marker.plugin_id == plugin_id
        {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Check if a specific failure key is known.
pub fn has_failure_marker(failures_root: &Path, key: &FailureKey) -> Result<bool> {
    let path = failures_root.join(key.filename());
    Ok(path.exists())
}

/// Current wall-clock timestamp for failure markers.
pub fn timestamp_now() -> String {
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{}s-since-epoch", now.as_secs())
}

/// Operation lock using fd-lock for cross-process mutual exclusion.
///
/// Uses `flock(LOCK_EX)` under the hood. The lock is released when the
/// guard is dropped (which closes the file descriptor).
pub struct OperationLock;

impl OperationLock {
    fn open_lock_file(lock_path: &Path) -> Result<fs::File> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .with_context(|| format!("failed to open lock file: {}", lock_path.display()))
    }

    /// Acquire the exclusive write lock, blocking until available.
    pub fn acquire(lock_path: &Path) -> Result<OperationLockGuard> {
        let file = Self::open_lock_file(lock_path)?;
        let mut lock = fd_lock::RwLock::new(file);
        let guard = lock.write().map_err(|e| anyhow::anyhow!("failed to acquire lock: {e}"))?;
        // Safety rationale: `fd_lock::RwLockWriteGuard` calls `LOCK_UN` on
        // drop, which would release the flock immediately.  We want the lock
        // held until `OperationLockGuard` (which owns the `RwLock<File>`) is
        // dropped — at that point the fd is closed and the OS releases the
        // flock.  `forget` is safe here because `RwLockWriteGuard` borrows the
        // same fd owned by `RwLock`; no separate resources are leaked.
        std::mem::forget(guard);
        Ok(OperationLockGuard { _lock: lock })
    }

    /// Try to acquire the exclusive write lock. Returns None if already held.
    pub fn try_acquire(lock_path: &Path) -> Result<Option<OperationLockGuard>> {
        let file = Self::open_lock_file(lock_path)?;
        let mut lock = fd_lock::RwLock::new(file);
        {
            let Some(guard) = map_try_write_result(lock.try_write())
                .map_err(|e| anyhow::anyhow!("failed to acquire lock: {e}"))?
            else {
                return Ok(None);
            };
            // Scope the borrow so we can move `lock` afterwards.
            {
                // See `acquire` for why `forget` is correct here.
                std::mem::forget(guard);
            }
        }
        Ok(Some(OperationLockGuard { _lock: lock }))
    }
}

pub struct OperationLockGuard {
    _lock: fd_lock::RwLock<fs::File>,
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind, Result};

    #[test]
    fn try_write_result_maps_would_block_to_none() {
        let result: Result<()> = Err(ErrorKind::WouldBlock.into());
        assert!(super::map_try_write_result(result).unwrap().is_none());
    }

    #[test]
    fn try_write_result_preserves_real_errors() {
        let err = super::map_try_write_result::<()>(Err(Error::other("boom"))).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Other);
    }
}
