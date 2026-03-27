use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::lockfile::{
    LockEntry, LockFile, TrackingRecord, config_fingerprint, remote_plugin_config_hash,
    write_lockfile_atomic,
};
use crate::model::{Config, PluginSource, Tracking};
use crate::planner::{self, PluginStatus, collect_failed_builds};
use crate::progress::{self, ProgressEvent, ProgressReporter, Stage};
use crate::state::{self, FailureKey, FailureMarker, Paths, build_command_hash, timestamp_now};
use crate::{git, prepare, repo, short_hash};

type UpdatePrepareResult = (PathBuf, String, TrackingRecord, Option<String>);

/// Install missing remote plugins. Lock-first: uses lock entry if present.
///
/// When `skip_known_failures` is true (legacy second-phase init install path),
/// plugins whose resolved commit matches an existing failure marker are skipped.
/// Main init reconciliation now performs known-failure suppression in `sync`.
pub async fn install(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    skip_known_failures: bool,
    reporter: &dyn ProgressReporter,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    // Phase 1: Select candidates and clean up broken dirs.
    let candidates: Vec<_> = config
        .plugins
        .iter()
        .filter(|spec| {
            let PluginSource::Remote { id, .. } = &spec.source else {
                return false;
            };
            if let Some(target) = target_id
                && id != target
            {
                return false;
            }
            let target_dir = paths.plugin_dir(id);
            let health = planner::inspect_plugin_dir(&target_dir);
            if matches!(health, planner::RepoHealth::Healthy { .. })
                && lock.plugins.contains_key(id.as_str())
            {
                return false;
            }
            if matches!(health, planner::RepoHealth::Broken) {
                let _ = std::fs::remove_dir_all(&target_dir);
            }
            true
        })
        .collect();

    // Phase 2: Parallel prepare — resolve and stage each candidate.
    let lock_ref = &*lock;
    let prepare_jobs: Vec<_> = candidates
        .iter()
        .map(|spec| {
            let PluginSource::Remote { id, clone_url, .. } = &spec.source else { unreachable!() };
            async move {
                reporter.report(ProgressEvent::PluginStage {
                    id,
                    name: &spec.name,
                    stage: Stage::Fetching,
                    detail: Some(clone_url.clone()),
                });
                if let Some(entry) = lock_ref.plugins.get(id.as_str()) {
                    let revision =
                        repo::ensure_locked_revision(paths, id, clone_url, &entry.commit).await?;
                    let prepared =
                        repo::materialize_staging_at_revision(paths, id, clone_url, &revision)
                            .await?;
                    Ok::<_, anyhow::Error>((
                        prepared.staging_dir,
                        entry.commit.clone(),
                        entry.tracking.clone(),
                    ))
                } else {
                    let revision =
                        repo::resolve_tracking_revision(paths, id, clone_url, &spec.tracking)
                            .await?;
                    let prepared =
                        repo::materialize_staging_at_revision(paths, id, clone_url, &revision)
                            .await?;
                    let commit = prepared.commit;
                    let record = prepared.tracking.expect("tracking metadata required");
                    reporter.report(ProgressEvent::PluginStage {
                        id,
                        name: &spec.name,
                        stage: Stage::Resolving,
                        detail: Some(repo::describe_tracking_resolution(
                            &spec.tracking,
                            &record,
                            &commit,
                        )),
                    });
                    Ok((prepared.staging_dir, commit, record))
                }
            }
        })
        .collect();
    let prepare_results = prepare::run_bounded(config.options.concurrency, prepare_jobs).await;

    // Phase 3: Serial apply in declaration order.
    for (spec, result) in candidates.iter().zip(prepare_results) {
        let PluginSource::Remote { id, .. } = &spec.source else { unreachable!() };
        let name = spec.name.as_str();

        let (staging, commit, tracking_record) = match result {
            Ok(v) => v,
            Err(e) => {
                let (summary, detail) = progress::summarize_error(&e);
                let context = remote_failure_context(spec, paths);
                reporter.report(ProgressEvent::PluginFailed {
                    id,
                    name,
                    stage: Some(Stage::Fetching),
                    summary,
                    detail,
                    context,
                });
                failures.push(format!("{id}: {e}"));
                continue;
            }
        };
        let config_hash = remote_plugin_config_hash(spec);

        if skip_known_failures
            && let Some(build_cmd) = &spec.build
            && is_known_failure(paths, id, &commit, build_cmd)?
        {
            reporter.report(ProgressEvent::PluginSkipped {
                id,
                name,
                reason: format!("known build failure at {}", short_hash(&commit)),
            });
            let _ = std::fs::remove_dir_all(&staging);
            continue;
        }

        reporter.report(ProgressEvent::PluginStage {
            id,
            name,
            stage: Stage::Applying,
            detail: spec.build.clone(),
        });

        let target_dir = paths.plugin_dir(id);
        match publish_and_track(paths, id, &commit, &staging, &target_dir, spec.build.as_deref()) {
            Ok(()) => {
                reporter.report(ProgressEvent::PluginDone {
                    id,
                    name,
                    summary: format!("installed {}", short_hash(&commit)),
                });
                lock.plugins.insert(
                    id.clone(),
                    LockEntry { tracking: tracking_record, commit, config_hash },
                );
            }
            Err(e) => {
                let (summary, detail) = progress::summarize_error(&e);
                let context =
                    remote_failure_context_with_revision(spec, paths, "resolved_commit", &commit);
                reporter.report(ProgressEvent::PluginFailed {
                    id,
                    name,
                    stage: Some(Stage::Applying),
                    summary,
                    detail,
                    context,
                });
                failures.push(format!("{id}: {e}"));
            }
        }
    }

    lock.config_fingerprint = Some(config_fingerprint(config));
    write_lockfile_atomic(&paths.lockfile_path, lock)?;
    if !failures.is_empty() {
        return Err(progress::progress_failure(format!(
            "{} plugin(s) failed to install:\n  {}",
            failures.len(),
            failures.join("\n  ")
        )));
    }
    Ok(())
}

/// Update remote plugins. The only command that advances lock.
pub async fn update(
    config: &Config,
    lock: &mut LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    reporter: &dyn ProgressReporter,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    // Phase 1: Select candidates — skip pinned plugins before scheduling work.
    let candidates: Vec<_> = config
        .plugins
        .iter()
        .filter(|spec| {
            let PluginSource::Remote { id, .. } = &spec.source else {
                return false;
            };
            if let Some(target) = target_id
                && id != target
            {
                return false;
            }
            let name = spec.name.as_str();
            match &spec.tracking {
                Tracking::Tag(t) => {
                    reporter.report(ProgressEvent::PluginSkipped {
                        id,
                        name,
                        reason: format!("pinned to tag {t}"),
                    });
                    false
                }
                Tracking::Commit(c) => {
                    reporter.report(ProgressEvent::PluginSkipped {
                        id,
                        name,
                        reason: format!("pinned to commit {}", short_hash(c)),
                    });
                    false
                }
                _ => true,
            }
        })
        .collect();

    // Phase 2: Parallel prepare — fetch, resolve, stage, read disk commit.
    let prepare_jobs: Vec<_> = candidates
        .iter()
        .map(|spec| {
            let PluginSource::Remote { id, clone_url, .. } = &spec.source else { unreachable!() };
            let target_dir = paths.plugin_dir(id);
            async move {
                reporter.report(ProgressEvent::PluginStage {
                    id,
                    name: &spec.name,
                    stage: Stage::Fetching,
                    detail: Some(clone_url.clone()),
                });
                let revision =
                    repo::resolve_tracking_revision(paths, id, clone_url, &spec.tracking).await?;
                let prepared =
                    repo::materialize_staging_at_revision(paths, id, clone_url, &revision).await?;
                let new_commit = prepared.commit;
                let record = prepared.tracking.expect("tracking metadata required");
                reporter.report(ProgressEvent::PluginStage {
                    id,
                    name: &spec.name,
                    stage: Stage::Resolving,
                    detail: Some(repo::describe_tracking_resolution(
                        &spec.tracking,
                        &record,
                        &new_commit,
                    )),
                });
                let disk_commit = if target_dir.exists() {
                    git::head_commit(&target_dir).await.ok()
                } else {
                    None
                };
                Ok::<_, anyhow::Error>((prepared.staging_dir, new_commit, record, disk_commit))
            }
        })
        .collect();
    let mut prepare_results: Vec<Option<Result<UpdatePrepareResult, anyhow::Error>>> =
        prepare::run_bounded(config.options.concurrency, prepare_jobs)
            .await
            .into_iter()
            .map(Some)
            .collect();

    // Phase 3: Serial apply in declaration order.
    for (idx, spec) in candidates.iter().enumerate() {
        let PluginSource::Remote { id, .. } = &spec.source else { unreachable!() };
        let name = spec.name.as_str();
        let config_hash = remote_plugin_config_hash(spec);
        let result = prepare_results[idx].take().expect("prepare result should exist");

        let (staging, new_commit, tracking_record, disk_commit) = match result {
            Ok(v) => v,
            Err(e) => {
                let (summary, detail) = progress::summarize_error(&e);
                let context = remote_failure_context(spec, paths);
                reporter.report(ProgressEvent::PluginFailed {
                    id,
                    name,
                    stage: Some(Stage::Fetching),
                    summary,
                    detail,
                    context,
                });
                failures.push(format!("{id}: {e}"));
                continue;
            }
        };

        let target_dir = paths.plugin_dir(id);
        let revision_changed = disk_commit.as_deref() != Some(new_commit.as_str());

        if !revision_changed && target_dir.exists() {
            let _ = std::fs::remove_dir_all(&staging);
            lock.plugins.insert(
                id.clone(),
                LockEntry { tracking: tracking_record, commit: new_commit, config_hash },
            );
            if let Err(err) = state::clear_failure_markers(&paths.failures_root, id) {
                cleanup_pending_update_staging(&mut prepare_results[idx + 1..]);
                return Err(err);
            }
            reporter.report(ProgressEvent::PluginDone {
                id,
                name,
                summary: "up-to-date".to_string(),
            });
            continue;
        }

        reporter.report(ProgressEvent::PluginStage {
            id,
            name,
            stage: Stage::Applying,
            detail: spec.build.clone(),
        });

        match publish_and_track(
            paths,
            id,
            &new_commit,
            &staging,
            &target_dir,
            spec.build.as_deref(),
        ) {
            Ok(()) => {
                let summary = match disk_commit.as_deref() {
                    Some(old) => {
                        format!("updated {} -> {}", short_hash(old), short_hash(&new_commit))
                    }
                    None => format!("installed {}", short_hash(&new_commit)),
                };
                reporter.report(ProgressEvent::PluginDone { id, name, summary });
                lock.plugins.insert(
                    id.clone(),
                    LockEntry { tracking: tracking_record, commit: new_commit, config_hash },
                );
            }
            Err(e) => {
                let (summary, detail) = progress::summarize_error(&e);
                let mut context = remote_failure_context_with_revision(
                    spec,
                    paths,
                    "resolved_commit",
                    &new_commit,
                );
                if let Some(old_commit) = disk_commit.as_deref() {
                    context.push(("previous_commit", old_commit.to_string()));
                }
                reporter.report(ProgressEvent::PluginFailed {
                    id,
                    name,
                    stage: Some(Stage::Applying),
                    summary,
                    detail,
                    context,
                });
                failures.push(format!("{id}: {e}"));
            }
        }
    }

    lock.config_fingerprint = Some(config_fingerprint(config));
    write_lockfile_atomic(&paths.lockfile_path, lock)?;
    if !failures.is_empty() {
        return Err(progress::progress_failure(format!(
            "{} plugin(s) failed to update:\n  {}",
            failures.len(),
            failures.join("\n  ")
        )));
    }
    Ok(())
}

/// Restore plugins to lock-recorded commits.
pub async fn restore(
    config: &Config,
    lock: &LockFile,
    paths: &Paths,
    target_id: Option<&str>,
    reporter: &dyn ProgressReporter,
) -> Result<()> {
    config.validate_target_id(target_id)?;
    paths.ensure_dirs()?;
    let mut failures: Vec<String> = Vec::new();

    // Phase 1: Select candidates — skip plugins without lock entries or already
    // at the correct commit.
    let candidates: Vec<_> = config
        .plugins
        .iter()
        .filter(|spec| {
            let PluginSource::Remote { id, .. } = &spec.source else {
                return false;
            };
            if let Some(target) = target_id
                && id != target
            {
                return false;
            }
            let name = spec.name.as_str();
            let Some(entry) = lock.plugins.get(id.as_str()) else {
                reporter.report(ProgressEvent::PluginSkipped {
                    id,
                    name,
                    reason: "no lock entry".to_string(),
                });
                return false;
            };
            let target_dir = paths.plugin_dir(id);
            let current_commit =
                if target_dir.exists() { git::head_commit_sync(&target_dir).ok() } else { None };
            let revision_changed = current_commit.as_deref() != Some(&entry.commit);
            if !revision_changed && target_dir.exists() {
                let _ = state::clear_failure_markers(&paths.failures_root, id);
                reporter.report(ProgressEvent::PluginDone {
                    id,
                    name,
                    summary: "already restored".to_string(),
                });
                return false;
            }
            true
        })
        .collect();

    // Phase 2: Parallel prepare — ensure locked revision and stage.
    let prepare_jobs: Vec<_> = candidates
        .iter()
        .map(|spec| {
            let PluginSource::Remote { id, clone_url, .. } = &spec.source else { unreachable!() };
            let entry = lock.plugins.get(id.as_str()).unwrap();
            async move {
                reporter.report(ProgressEvent::PluginStage {
                    id,
                    name: &spec.name,
                    stage: Stage::Fetching,
                    detail: Some(clone_url.clone()),
                });
                let revision =
                    repo::ensure_locked_revision(paths, id, clone_url, &entry.commit).await?;
                let prepared =
                    repo::materialize_staging_at_revision(paths, id, clone_url, &revision).await?;
                Ok::<_, anyhow::Error>(prepared.staging_dir)
            }
        })
        .collect();
    let prepare_results = prepare::run_bounded(config.options.concurrency, prepare_jobs).await;

    // Phase 3: Serial apply in declaration order.
    for (spec, result) in candidates.iter().zip(prepare_results) {
        let PluginSource::Remote { id, .. } = &spec.source else { unreachable!() };
        let name = spec.name.as_str();
        let entry = lock.plugins.get(id.as_str()).unwrap();

        let staging = match result {
            Ok(v) => v,
            Err(e) => {
                let (summary, detail) = progress::summarize_error(&e);
                let mut context = remote_failure_context(spec, paths);
                context.push(("locked_commit", entry.commit.clone()));
                reporter.report(ProgressEvent::PluginFailed {
                    id,
                    name,
                    stage: Some(Stage::Fetching),
                    summary,
                    detail,
                    context,
                });
                failures.push(format!("{id}: {e}"));
                continue;
            }
        };

        reporter.report(ProgressEvent::PluginStage {
            id,
            name,
            stage: Stage::Applying,
            detail: spec.build.clone(),
        });

        let target_dir = paths.plugin_dir(id);
        if let Err(e) = publish_and_track(
            paths,
            id,
            &entry.commit,
            &staging,
            &target_dir,
            spec.build.as_deref(),
        ) {
            let (summary, detail) = progress::summarize_error(&e);
            let mut context = remote_failure_context(spec, paths);
            context.push(("locked_commit", entry.commit.clone()));
            reporter.report(ProgressEvent::PluginFailed {
                id,
                name,
                stage: Some(Stage::Applying),
                summary,
                detail,
                context,
            });
            failures.push(format!("{id}: {e}"));
        } else {
            reporter.report(ProgressEvent::PluginDone {
                id,
                name,
                summary: format!("restored {}", short_hash(&entry.commit)),
            });
        }
    }

    if !failures.is_empty() {
        return Err(progress::progress_failure(format!(
            "{} plugin(s) failed to restore:\n  {}",
            failures.len(),
            failures.join("\n  ")
        )));
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
    let health_map = planner::build_health_map(config, paths);
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

/// Publish a plugin from staging to its target directory, tracking success/failure.
///
/// On success: clears failure markers for the plugin.
/// On failure: records a failure marker (if a build command is configured),
/// cleans up the staging directory, and returns the error.
fn publish_and_track(
    paths: &Paths,
    id: &str,
    commit: &str,
    staging: &Path,
    target: &Path,
    build: Option<&str>,
) -> Result<()> {
    let result = if target.exists() {
        git::publish_replace(staging, target, build)
    } else {
        git::publish_fresh_install(staging, target, build)
    };

    match result {
        Ok(()) => {
            state::clear_failure_markers(&paths.failures_root, id)?;
            Ok(())
        }
        Err(e) => {
            if let Some(build_cmd) = build {
                let bh = build_command_hash(build_cmd);
                let marker = FailureMarker {
                    plugin_id: id.to_string(),
                    commit: commit.to_string(),
                    build_hash: bh,
                    build_command: build_cmd.to_string(),
                    failed_at: timestamp_now(),
                    stderr_summary: format!("{e}"),
                };
                let _ = state::write_failure_marker(&paths.failures_root, &marker);
            }
            let _ = std::fs::remove_dir_all(staging);
            Err(e)
        }
    }
}

fn cleanup_pending_update_staging(
    pending: &mut [Option<Result<UpdatePrepareResult, anyhow::Error>>],
) {
    for item in pending {
        if let Some(Ok((staging, ..))) = item.take() {
            let _ = std::fs::remove_dir_all(staging);
        }
    }
}

fn tracking_selector(tracking: &Tracking) -> String {
    match tracking {
        Tracking::DefaultBranch => "default-branch".to_string(),
        Tracking::Branch(branch) => format!("branch:{branch}"),
        Tracking::Tag(tag) => format!("tag:{tag}"),
        Tracking::Commit(commit) => format!("commit:{commit}"),
    }
}

fn remote_failure_context(
    spec: &crate::model::PluginSpec,
    paths: &Paths,
) -> Vec<(&'static str, String)> {
    let PluginSource::Remote { id, clone_url, .. } = &spec.source else {
        return vec![];
    };
    vec![
        ("clone_url", clone_url.clone()),
        ("tracking", tracking_selector(&spec.tracking)),
        ("target_dir", paths.plugin_dir(id).display().to_string()),
    ]
}

fn remote_failure_context_with_revision(
    spec: &crate::model::PluginSpec,
    paths: &Paths,
    key: &'static str,
    revision: &str,
) -> Vec<(&'static str, String)> {
    let mut context = remote_failure_context(spec, paths);
    context.push((key, revision.to_string()));
    context
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
