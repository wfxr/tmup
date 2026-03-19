use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::lockfile::{
    LockEntry, LockFile, TrackingRecord, config_fingerprint, remote_plugin_config_hash,
    write_lockfile_atomic,
};
use crate::model::{Config, PluginSource, PluginSpec, Tracking};
use crate::state::{self, FailureMarker, Paths, build_command_hash, timestamp_now};
use crate::{git, planner, plugin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncPolicy {
    pub install_new_plugins: bool,
    pub repair_existing_plugins: bool,
    pub prune_removed_plugins: bool,
}

impl SyncPolicy {
    pub const SYNC: Self = Self {
        install_new_plugins: true,
        repair_existing_plugins: true,
        prune_removed_plugins: true,
    };
    pub const INSTALL: Self = Self::SYNC;
    pub const UPDATE: Self = Self::SYNC;
    pub const RESTORE: Self = Self::SYNC;
    pub const CLEAN: Self = Self {
        install_new_plugins: false,
        repair_existing_plugins: false,
        prune_removed_plugins: true,
    };

    pub fn init(auto_install: bool) -> Self {
        Self {
            install_new_plugins: auto_install,
            repair_existing_plugins: true,
            prune_removed_plugins: true,
        }
    }
}

pub async fn run(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    policy: SyncPolicy,
) -> Result<()> {
    config.validate_target_id(target_id)?;

    let desired_hashes = desired_remote_hashes(config);
    let mut failures = Vec::new();

    for spec in config.plugins.iter().filter(|spec| match spec.remote_id() {
        Some(id) => target_id.is_none_or(|target| target == id),
        None => false,
    }) {
        let id = spec.remote_id().unwrap();
        let desired_hash = desired_hashes.get(id).cloned().unwrap();

        if lock.plugins.get(id).and_then(|entry| entry.config_hash.as_deref())
            == Some(desired_hash.as_str())
        {
            continue;
        }

        let is_new = !lock.plugins.contains_key(id);
        let can_reconcile =
            if is_new { policy.install_new_plugins } else { policy.repair_existing_plugins };
        if !can_reconcile {
            continue;
        }

        let current_entry = lock.plugins.get(id);
        match resolve_desired_plugin(spec, current_entry, desired_hash.clone(), paths).await {
            Ok(resolved) => {
                if let Err(err) = reconcile_plugin(spec, lock, paths, resolved).await {
                    failures.push(format!("{id}: {err}"));
                }
            }
            Err(err) => failures.push(format!("{id}: {err}")),
        }
    }

    if target_id.is_none() && policy.prune_removed_plugins {
        let desired_ids: std::collections::HashSet<_> = desired_hashes.keys().cloned().collect();
        lock.plugins.retain(|id, _| desired_ids.contains(id));
    }

    if lock_matches_config(config, lock) {
        lock.config_fingerprint = Some(config_fingerprint(config));
    }

    if !failures.is_empty() {
        anyhow::bail!("{} plugin(s) failed to sync:\n  {}", failures.len(), failures.join("\n  "));
    }

    Ok(())
}

pub async fn run_and_write(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    policy: SyncPolicy,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    let sync_result = run(config, lock, paths, target_id, policy).await;
    write_lockfile_atomic(&paths.lockfile_path, lock)?;
    sync_result
}

pub fn lock_is_stale(config: &Config, lock: &LockFile) -> bool {
    let has_remote_plugins = config.plugins.iter().any(|spec| spec.remote_id().is_some());
    if !has_remote_plugins {
        return !lock.plugins.is_empty();
    }

    let expected_fingerprint = config_fingerprint(config);
    if lock.config_fingerprint.as_deref() != Some(expected_fingerprint.as_str()) {
        return true;
    }

    !lock_matches_config(config, lock)
}

pub fn lock_matches_config(config: &Config, lock: &LockFile) -> bool {
    let desired = desired_remote_hashes(config);
    if lock.plugins.len() != desired.len() {
        return false;
    }

    desired.into_iter().all(|(id, expected_hash)| {
        lock.plugins.get(&id).and_then(|entry| entry.config_hash.as_deref())
            == Some(expected_hash.as_str())
    })
}

fn desired_remote_hashes(config: &Config) -> BTreeMap<String, String> {
    config
        .plugins
        .iter()
        .filter_map(|spec| Some((spec.remote_id()?.to_string(), remote_plugin_config_hash(spec)?)))
        .collect()
}

struct ResolvedPlugin {
    id: String,
    staging_dir: PathBuf,
    entry: LockEntry,
}

async fn resolve_desired_plugin(
    spec: &PluginSpec,
    current_entry: Option<&LockEntry>,
    config_hash: String,
    paths: &Paths,
) -> Result<ResolvedPlugin> {
    let PluginSource::Remote { id, clone_url, .. } = &spec.source else {
        unreachable!("sync only processes remote plugins");
    };

    let staging_dir = paths.staging_dir(id);
    let prep = async {
        git::clone_repo(clone_url, &staging_dir).await?;

        let (commit, tracking) = if let Some(entry) = current_entry
            && tracks_same_revision(spec, &entry.tracking)
        {
            (entry.commit.clone(), entry.tracking.clone())
        } else {
            plugin::resolve_tracking(&staging_dir, &spec.tracking).await?
        };
        git::checkout(&staging_dir, &commit).await?;
        Ok::<_, anyhow::Error>(LockEntry { tracking, commit, config_hash: Some(config_hash) })
    }
    .await;

    match prep {
        Ok(entry) => Ok(ResolvedPlugin { id: id.clone(), staging_dir, entry }),
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging_dir);
            Err(err)
        }
    }
}

fn tracks_same_revision(spec: &PluginSpec, locked: &TrackingRecord) -> bool {
    match &spec.tracking {
        Tracking::DefaultBranch => locked.kind == "default-branch",
        Tracking::Branch(branch) => locked.kind == "branch" && locked.value == *branch,
        Tracking::Tag(tag) => locked.kind == "tag" && locked.value == *tag,
        Tracking::Commit(commit) => locked.kind == "commit" && locked.value == *commit,
    }
}

async fn reconcile_plugin(
    spec: &PluginSpec,
    lock: &mut LockFile,
    paths: &Paths,
    resolved: ResolvedPlugin,
) -> Result<()> {
    let ResolvedPlugin { id, staging_dir, entry } = resolved;

    let target_dir = paths.plugin_dir(&id);
    let current_entry = lock.plugins.get(&id).cloned();
    let health = planner::inspect_plugin_dir(&target_dir);

    let current_commit = match &health {
        planner::RepoHealth::Healthy { commit } => Some(commit.clone()),
        planner::RepoHealth::Missing | planner::RepoHealth::Broken => None,
    };

    if matches!(health, planner::RepoHealth::Broken) {
        let _ = std::fs::remove_dir_all(&target_dir);
    }

    let same_commit = current_commit.as_deref() == Some(entry.commit.as_str());
    let needs_republish =
        same_commit_config_change_requires_republish(current_entry.as_ref(), &entry);
    let needs_publish = current_entry.is_none()
        || matches!(health, planner::RepoHealth::Missing | planner::RepoHealth::Broken)
        || !same_commit
        || needs_republish;

    if !needs_publish {
        let _ = std::fs::remove_dir_all(&staging_dir);
        state::clear_failure_markers(&paths.failures_root, &id)?;
        lock.plugins.insert(id, entry);
        return Ok(());
    }

    let build = spec.build.as_deref();
    let publish_result = if target_dir.exists() {
        let backup = paths.backup_dir(&id);
        git::publish_replace(&staging_dir, &target_dir, &backup, build)
    } else {
        git::publish_fresh_install(&staging_dir, &target_dir, build)
    };

    match publish_result {
        Ok(()) => {
            state::clear_failure_markers(&paths.failures_root, &id)?;
            lock.plugins.insert(id, entry);
            Ok(())
        }
        Err(err) => {
            record_failure_marker(paths, &id, &entry.commit, spec, &err)?;
            Err(err)
        }
    }
}

fn same_commit_config_change_requires_republish(
    current_entry: Option<&LockEntry>,
    desired_entry: &LockEntry,
) -> bool {
    let Some(current_entry) = current_entry else {
        return false;
    };
    if current_entry.commit != desired_entry.commit {
        return false;
    }
    if current_entry.config_hash == desired_entry.config_hash {
        return false;
    }
    true
}

fn record_failure_marker(
    paths: &Paths,
    id: &str,
    commit: &str,
    spec: &PluginSpec,
    err: &anyhow::Error,
) -> Result<()> {
    let Some(build_cmd) = &spec.build else {
        return Ok(());
    };

    let marker = FailureMarker {
        plugin_id: id.to_string(),
        commit: commit.to_string(),
        build_hash: build_command_hash(build_cmd),
        build_command: build_cmd.clone(),
        failed_at: timestamp_now(),
        stderr_summary: format!("{err}"),
    };
    state::write_failure_marker(&paths.failures_root, &marker)
}
