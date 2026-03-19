use std::collections::HashSet;

use anyhow::{Context, Result};

use crate::git;
use crate::lockfile::{
    LockEntry, LockFile, TrackingRecord, config_fingerprint, remote_plugin_config_hash,
    write_lockfile_atomic,
};
use crate::model::{Config, PluginSource, Tracking};
use crate::planner::{self, PluginStatus, collect_failed_builds};
use crate::state::{self, FailureKey, FailureMarker, Paths, build_command_hash, timestamp_now};

/// Install missing remote plugins. Lock-first: uses lock entry if present.
///
/// When `skip_known_failures` is true (used by init auto-install), plugins whose
/// resolved commit matches an existing failure marker are silently skipped.
pub async fn install(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    skip_known_failures: bool,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    for spec in &config.plugins {
        let PluginSource::Remote { id, clone_url, .. } = &spec.source else {
            continue;
        };

        if let Some(target) = target_id
            && id != target
        {
            continue;
        }

        let target_dir = paths.plugin_dir(id);
        let health = planner::inspect_plugin_dir(&target_dir);
        if matches!(health, planner::RepoHealth::Healthy { .. })
            && lock.plugins.contains_key(id.as_str())
        {
            continue;
        }

        // If Broken, remove the broken dir before proceeding
        if matches!(health, planner::RepoHealth::Broken) {
            let _ = std::fs::remove_dir_all(&target_dir);
        }

        let staging = paths.staging_dir(id);

        // Determine target commit — git failures are per-plugin, not fatal.
        let prep: Result<_> = async {
            if let Some(entry) = lock.plugins.get(id.as_str()) {
                git::clone_repo(clone_url, &staging).await?;
                git::checkout(&staging, &entry.commit).await?;
                Ok((entry.commit.clone(), entry.tracking.clone()))
            } else {
                git::clone_repo(clone_url, &staging).await?;
                let (commit, record) = resolve_tracking(&staging, &spec.tracking).await?;
                git::checkout(&staging, &commit).await?;
                Ok((commit, record))
            }
        }
        .await;

        let (commit, tracking_record) = match prep {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                let msg = format!("{id}: {e}");
                eprintln!("failed to install {msg}");
                failures.push(msg);
                continue;
            }
        };
        let config_hash = remote_plugin_config_hash(spec);

        // Check known failure suppression (for init auto-install)
        if skip_known_failures
            && let Some(build_cmd) = &spec.build
            && is_known_failure(paths, id, &commit, build_cmd)?
        {
            eprintln!(
                "lazytmux: skipping {id} (known build failure at {})",
                &commit[..7.min(commit.len())]
            );
            let _ = std::fs::remove_dir_all(&staging);
            continue;
        }

        // Publish
        let result = if target_dir.exists() {
            let backup = paths.backup_dir(id);
            git::publish_replace(&staging, &target_dir, &backup, spec.build.as_deref())
        } else {
            git::publish_fresh_install(&staging, &target_dir, spec.build.as_deref())
        };
        match result {
            Ok(()) => {
                // Update lock
                lock.plugins.insert(
                    id.clone(),
                    LockEntry {
                        source: id.clone(),
                        tracking: tracking_record,
                        commit,
                        config_hash,
                    },
                );
                // Clear failure markers on success
                state::clear_failure_markers(&paths.failures_root, id)?;
            }
            Err(e) => {
                // Record failure marker
                if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    let marker = FailureMarker {
                        plugin_id: id.clone(),
                        commit: commit.clone(),
                        build_hash: bh,
                        build_command: build_cmd.clone(),
                        failed_at: timestamp_now(),
                        stderr_summary: format!("{e}"),
                    };
                    let _ = state::write_failure_marker(&paths.failures_root, &marker);
                }
                // Clean up staging if it still exists
                let _ = std::fs::remove_dir_all(&staging);
                let msg = format!("{id}: {e}");
                eprintln!("failed to install {msg}");
                failures.push(msg);
            }
        }
    }

    lock.config_fingerprint = Some(config_fingerprint(config));
    write_lockfile_atomic(&paths.lockfile_path, lock)?;
    if !failures.is_empty() {
        anyhow::bail!(
            "{} plugin(s) failed to install:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
    Ok(())
}

/// Update remote plugins. The only command that advances lock.
pub async fn update(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    for spec in &config.plugins {
        let PluginSource::Remote { id, clone_url, .. } = &spec.source else {
            continue;
        };

        if let Some(target) = target_id
            && id != target
        {
            continue;
        }

        // Skip pinned plugins
        match &spec.tracking {
            Tracking::Tag(t) => {
                eprintln!("{id}: pinned to tag {t}, skipping update");
                continue;
            }
            Tracking::Commit(c) => {
                eprintln!("{id}: pinned to commit {c}, skipping update");
                continue;
            }
            _ => {}
        }

        let target_dir = paths.plugin_dir(id);
        let staging = paths.staging_dir(id);
        let config_hash = remote_plugin_config_hash(spec);

        // Git preparation — failures are per-plugin, not fatal.
        let prep = async {
            git::clone_repo(clone_url, &staging).await?;
            git::fetch(&staging).await?;
            let (new_commit, record) = resolve_tracking(&staging, &spec.tracking).await?;
            git::checkout(&staging, &new_commit).await?;
            Ok::<_, anyhow::Error>((new_commit, record))
        }
        .await;

        let (new_commit, tracking_record) = match prep {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                let msg = format!("{id}: {e}");
                eprintln!("failed to update {msg}");
                failures.push(msg);
                continue;
            }
        };

        // Check if the disk HEAD already matches new_commit.
        let disk_commit =
            if target_dir.exists() { git::head_commit(&target_dir).await.ok() } else { None };
        let revision_changed = disk_commit.as_deref() != Some(new_commit.as_str());

        // Already at the target commit — just update the lock, skip publish.
        if !revision_changed && target_dir.exists() {
            let _ = std::fs::remove_dir_all(&staging);
            lock.plugins.insert(
                id.clone(),
                LockEntry {
                    source: id.clone(),
                    tracking: tracking_record,
                    commit: new_commit,
                    config_hash: config_hash.clone(),
                },
            );
            // A no-op update is still a successful operation — clear any
            // stale failure markers so `list` doesn't show build-failed.
            state::clear_failure_markers(&paths.failures_root, id)?;
            continue;
        }

        // Revision changed or target missing — always run build if declared.
        let build = spec.build.as_deref();

        // Publish
        let result = if target_dir.exists() {
            let backup = paths.backup_dir(id);
            git::publish_replace(&staging, &target_dir, &backup, build)
        } else {
            git::publish_fresh_install(&staging, &target_dir, build)
        };

        match result {
            Ok(()) => {
                lock.plugins.insert(
                    id.clone(),
                    LockEntry {
                        source: id.clone(),
                        tracking: tracking_record,
                        commit: new_commit,
                        config_hash,
                    },
                );
                state::clear_failure_markers(&paths.failures_root, id)?;
            }
            Err(e) => {
                if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    let marker = FailureMarker {
                        plugin_id: id.clone(),
                        commit: new_commit.clone(),
                        build_hash: bh,
                        build_command: build_cmd.clone(),
                        failed_at: timestamp_now(),
                        stderr_summary: format!("{e}"),
                    };
                    let _ = state::write_failure_marker(&paths.failures_root, &marker);
                }
                let _ = std::fs::remove_dir_all(&staging);
                let msg = format!("{id}: {e}");
                eprintln!("failed to update {msg}");
                failures.push(msg);
            }
        }
    }

    lock.config_fingerprint = Some(config_fingerprint(config));
    write_lockfile_atomic(&paths.lockfile_path, lock)?;
    if !failures.is_empty() {
        anyhow::bail!(
            "{} plugin(s) failed to update:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
    Ok(())
}

/// Restore plugins to lock-recorded commits.
pub async fn restore(
    config: &Config,
    lock: &LockFile,
    paths: &Paths,
    target_id: Option<&str>,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    for spec in &config.plugins {
        let PluginSource::Remote { id, clone_url, .. } = &spec.source else {
            continue;
        };

        if let Some(target) = target_id
            && id != target
        {
            continue;
        }

        let Some(entry) = lock.plugins.get(id.as_str()) else {
            eprintln!("{id}: no lock entry, skipping restore");
            continue;
        };

        let target_dir = paths.plugin_dir(id);

        // Check if revision would actually change
        let current_commit =
            if target_dir.exists() { git::head_commit(&target_dir).await.ok() } else { None };
        let revision_changed = current_commit.as_deref() != Some(&entry.commit);

        // Already at the correct commit — nothing to do
        if !revision_changed && target_dir.exists() {
            // A no-op restore is still a successful operation — clear any
            // stale failure markers so `list` doesn't show build-failed.
            state::clear_failure_markers(&paths.failures_root, id)?;
            continue;
        }

        let staging = paths.staging_dir(id);

        // Clone into staging and checkout lock commit — per-plugin failure.
        let prep: Result<()> = async {
            git::clone_repo(clone_url, &staging).await?;
            git::checkout(&staging, &entry.commit).await?;
            Ok(())
        }
        .await;

        if let Err(e) = prep {
            let _ = std::fs::remove_dir_all(&staging);
            let msg = format!("{id}: {e}");
            eprintln!("failed to restore {msg}");
            failures.push(msg);
            continue;
        }

        // Revision changed (or missing), so run build if declared
        let build = spec.build.as_deref();

        let result = if target_dir.exists() {
            let backup = paths.backup_dir(id);
            git::publish_replace(&staging, &target_dir, &backup, build)
        } else {
            git::publish_fresh_install(&staging, &target_dir, build)
        };

        match result {
            Ok(()) => {
                state::clear_failure_markers(&paths.failures_root, id)?;
            }
            Err(e) => {
                if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    let marker = FailureMarker {
                        plugin_id: id.clone(),
                        commit: entry.commit.clone(),
                        build_hash: bh,
                        build_command: build_cmd.clone(),
                        failed_at: timestamp_now(),
                        stderr_summary: format!("{e}"),
                    };
                    let _ = state::write_failure_marker(&paths.failures_root, &marker);
                }
                let _ = std::fs::remove_dir_all(&staging);
                let msg = format!("{id}: {e}");
                eprintln!("failed to restore {msg}");
                failures.push(msg);
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} plugin(s) failed to restore:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
    Ok(())
}

/// Remove undeclared managed remote plugins.
pub fn clean(config: &Config, paths: &Paths) -> Result<()> {
    let managed_ids = planner::scan_managed_plugin_ids(&paths.plugin_root);
    let declared_ids: HashSet<&str> = config.plugins.iter().filter_map(|p| p.remote_id()).collect();

    let mut undeclared: Vec<&str> = managed_ids
        .iter()
        .filter(|id| !declared_ids.contains(id.as_str()))
        .map(|s| s.as_str())
        .collect();
    undeclared.sort();
    for id in undeclared {
        let dir = paths.plugin_dir(id);
        eprintln!("removing undeclared plugin: {id}");
        remove_managed_entry(&dir)?;
        // Clean up empty parent directories
        cleanup_empty_parents(&dir, &paths.plugin_root);
    }

    Ok(())
}

/// List plugin statuses.
pub fn list(config: &Config, lock: &LockFile, paths: &Paths) -> Result<Vec<PluginStatus>> {
    // Build health map from declared plugins
    let health_map: std::collections::HashMap<String, planner::RepoHealth> = config
        .plugins
        .iter()
        .filter_map(|spec| {
            let id = spec.remote_id()?;
            let health = planner::inspect_plugin_dir(&paths.plugin_dir(id));
            Some((id.to_string(), health))
        })
        .collect();
    let markers = state::read_failure_markers(&paths.failures_root)?;
    let failed_builds = collect_failed_builds(&markers);

    Ok(planner::compute_statuses(config, lock, &health_map, &failed_builds))
}

/// Check if a build failure key is known (for init auto-retry suppression).
pub fn is_known_failure(
    paths: &Paths,
    plugin_id: &str,
    commit: &str,
    build_cmd: &str,
) -> Result<bool> {
    let bh = build_command_hash(build_cmd);
    let key = FailureKey::new(plugin_id, commit, &bh);
    state::has_failure_marker(&paths.failures_root, &key)
}

pub(crate) async fn resolve_tracking(
    repo: &std::path::Path,
    tracking: &Tracking,
) -> Result<(String, TrackingRecord)> {
    match tracking {
        Tracking::Branch(branch) => {
            let commit = git::resolve_remote_branch(repo, branch).await?;
            Ok((commit, TrackingRecord { kind: "branch".into(), value: branch.clone() }))
        }
        Tracking::Tag(tag) => {
            git::checkout(repo, tag).await?;
            let commit = git::head_commit(repo).await?;
            Ok((commit, TrackingRecord { kind: "tag".into(), value: tag.clone() }))
        }
        Tracking::Commit(c) => {
            Ok((c.clone(), TrackingRecord { kind: "commit".into(), value: c.clone() }))
        }
        Tracking::DefaultBranch => {
            let branch = git::default_branch(repo).await?;
            let commit = git::resolve_remote_branch(repo, &branch).await?;
            Ok((commit, TrackingRecord { kind: "default-branch".into(), value: branch }))
        }
    }
}

fn cleanup_empty_parents(path: &std::path::Path, stop_at: &std::path::Path) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == stop_at {
            break;
        }
        if std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(dir);
            current = dir.parent();
        } else {
            break;
        }
    }
}

fn remove_managed_entry(path: &std::path::Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove symlink {}", path.display()))?;
    } else {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}
