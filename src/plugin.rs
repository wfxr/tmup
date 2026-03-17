use anyhow::{Context, Result};
use std::collections::HashSet;

use crate::{
    git,
    lockfile::{LockEntry, LockFile, TrackingRecord, write_lockfile_atomic},
    model::{Config, PluginSource, Tracking},
    planner::{self, PluginStatus, collect_failed_builds},
    state::{self, FailureKey, FailureMarker, Paths, build_command_hash},
};

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
    paths.ensure_dirs()?;
    let installed = planner::scan_installed_plugins(&paths.plugin_root);
    let mut failures: Vec<String> = Vec::new();

    for spec in &config.plugins {
        let PluginSource::Remote { raw, id, clone_url } = &spec.source else {
            continue;
        };

        if let Some(target) = target_id
            && id != target
        {
            continue;
        }

        if installed.contains_key(id.as_str()) {
            continue;
        }

        let staging = paths.staging_dir(id);
        let target_dir = paths.plugin_dir(id);

        // Determine target commit
        let (commit, tracking_record) = if let Some(entry) = lock.plugins.get(id.as_str()) {
            // Lock entry exists: install exact commit
            git::clone_repo(clone_url, &staging).await?;
            git::checkout(&staging, &entry.commit).await?;
            (entry.commit.clone(), entry.tracking.clone())
        } else {
            // No lock entry: resolve from config
            git::clone_repo(clone_url, &staging).await?;
            let (commit, record) = resolve_tracking(&staging, &spec.tracking).await?;
            git::checkout(&staging, &commit).await?;
            (commit, record)
        };

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
        let result = git::publish_fresh_install(&staging, &target_dir, spec.build.as_deref());
        match result {
            Ok(()) => {
                // Update lock
                lock.plugins.insert(id.clone(), LockEntry {
                    source: raw.clone(),
                    tracking: tracking_record,
                    commit,
                });
                // Clear failure markers on success
                state::clear_failure_markers(&paths.failures_root, id)?;
            }
            Err(e) => {
                // Record failure marker
                if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    let marker = FailureMarker {
                        plugin_id:      id.clone(),
                        commit:         commit.clone(),
                        build_hash:     bh,
                        build_command:  build_cmd.clone(),
                        failed_at:      chrono_now(),
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
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    for spec in &config.plugins {
        let PluginSource::Remote { raw, id, clone_url } = &spec.source else {
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

        // Clone or fetch into staging
        git::clone_repo(clone_url, &staging).await?;
        git::fetch(&staging).await?;

        // Resolve new commit
        let (new_commit, tracking_record) = resolve_tracking(&staging, &spec.tracking).await?;
        git::checkout(&staging, &new_commit).await?;

        // Check if revision actually changed
        let old_commit = lock.plugins.get(id.as_str()).map(|e| e.commit.as_str());
        let revision_changed = old_commit != Some(new_commit.as_str());

        let build = if revision_changed {
            spec.build.as_deref()
        } else {
            None
        };

        // Publish
        let result = if target_dir.exists() {
            let backup = paths.backup_dir(id);
            git::publish_replace(&staging, &target_dir, &backup, build)
        } else {
            git::publish_fresh_install(&staging, &target_dir, build)
        };

        match result {
            Ok(()) => {
                lock.plugins.insert(id.clone(), LockEntry {
                    source:   raw.clone(),
                    tracking: tracking_record,
                    commit:   new_commit,
                });
                state::clear_failure_markers(&paths.failures_root, id)?;
            }
            Err(e) => {
                if let Some(build_cmd) = &spec.build {
                    let bh = build_command_hash(build_cmd);
                    let marker = FailureMarker {
                        plugin_id:      id.clone(),
                        commit:         new_commit.clone(),
                        build_hash:     bh,
                        build_command:  build_cmd.clone(),
                        failed_at:      chrono_now(),
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
        let current_commit = if target_dir.exists() {
            git::head_commit(&target_dir).await.ok()
        } else {
            None
        };
        let revision_changed = current_commit.as_deref() != Some(&entry.commit);

        // Already at the correct commit — nothing to do
        if !revision_changed && target_dir.exists() {
            continue;
        }

        let staging = paths.staging_dir(id);

        // Clone into staging and checkout lock commit
        git::clone_repo(clone_url, &staging).await?;
        git::checkout(&staging, &entry.commit).await?;

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
                        plugin_id:      id.clone(),
                        commit:         entry.commit.clone(),
                        build_hash:     bh,
                        build_command:  build_cmd.clone(),
                        failed_at:      chrono_now(),
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
    let installed = planner::scan_installed_plugins(&paths.plugin_root);
    let declared_ids: HashSet<&str> = config
        .plugins
        .iter()
        .filter_map(|p| p.remote_id())
        .collect();

    for id in installed.keys() {
        if !declared_ids.contains(id.as_str()) {
            let dir = paths.plugin_dir(id);
            eprintln!("removing undeclared plugin: {id}");
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
            // Clean up empty parent directories
            cleanup_empty_parents(&dir, &paths.plugin_root);
        }
    }

    Ok(())
}

/// List plugin statuses.
pub fn list(config: &Config, lock: &LockFile, paths: &Paths) -> Result<Vec<PluginStatus>> {
    let installed = planner::scan_installed_plugins(&paths.plugin_root);
    let markers = state::read_failure_markers(&paths.failures_root)?;
    let failed_builds = collect_failed_builds(&markers);

    Ok(planner::compute_statuses(
        config,
        lock,
        &installed,
        &failed_builds,
    ))
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

async fn resolve_tracking(
    repo: &std::path::Path,
    tracking: &Tracking,
) -> Result<(String, TrackingRecord)> {
    match tracking {
        Tracking::Branch(branch) => {
            let commit = git::resolve_remote_branch(repo, branch).await?;
            Ok((commit, TrackingRecord {
                kind:  "branch".into(),
                value: branch.clone(),
            }))
        }
        Tracking::Tag(tag) => {
            git::checkout(repo, tag).await?;
            let commit = git::head_commit(repo).await?;
            Ok((commit, TrackingRecord {
                kind:  "tag".into(),
                value: tag.clone(),
            }))
        }
        Tracking::Commit(c) => Ok((c.clone(), TrackingRecord {
            kind:  "commit".into(),
            value: c.clone(),
        })),
        Tracking::DefaultBranch => {
            let branch = git::default_branch(repo).await?;
            let commit = git::resolve_remote_branch(repo, &branch).await?;
            Ok((commit, TrackingRecord {
                kind:  "branch".into(),
                value: branch,
            }))
        }
    }
}

fn cleanup_empty_parents(path: &std::path::Path, stop_at: &std::path::Path) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == stop_at {
            break;
        }
        if std::fs::read_dir(dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(dir);
            current = dir.parent();
        } else {
            break;
        }
    }
}

fn chrono_now() -> String {
    // Simple ISO-ish timestamp without chrono dependency
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s-since-epoch", now.as_secs())
}
