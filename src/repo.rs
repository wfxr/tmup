use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::lockfile::TrackingRecord;
use crate::model::Tracking;
use crate::state::Paths;
use crate::{git, short_hash};

const CACHE_FILTER: Option<git::ObjectFilter> = Some(git::ObjectFilter::BlobNone);

/// A prepared working repository cloned from the persistent cache.
pub struct PreparedRepo {
    /// Staging checkout ready for further operations.
    pub staging_dir: PathBuf,
    /// Resolved commit checked out in staging.
    pub commit: String,
    /// Tracking metadata resolved for floating selectors.
    pub tracking: Option<TrackingRecord>,
}

/// A revision that has been resolved in the persistent cache.
#[derive(Debug, Clone)]
pub struct ResolvedRevision {
    /// Resolved commit to materialize.
    pub commit: String,
    /// Tracking metadata for floating selectors.
    pub tracking: Option<TrackingRecord>,
}

/// Ensure a persistent bare cache exists for the remote plugin.
pub async fn ensure_cache_repo(paths: &Paths, plugin_id: &str, clone_url: &str) -> Result<bool> {
    let cache_dir = paths.repo_cache_dir(plugin_id);
    if git::head_commit(&cache_dir).await.is_ok() {
        git::set_remote_url(&cache_dir, "origin", clone_url).await?;
        return Ok(false);
    }
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir)?;
    }
    git::clone_bare_repo_filtered(clone_url, &cache_dir, CACHE_FILTER).await?;
    Ok(true)
}

/// Fetch refs needed for the given tracking strategy into the cache repo.
pub async fn fetch_for_tracking(repo: &Path, tracking: &Tracking) -> Result<()> {
    let refspecs = match tracking {
        Tracking::Branch(branch) => {
            vec![format!("refs/heads/{branch}:refs/remotes/origin/{branch}")]
        }
        Tracking::Tag(tag) => vec![format!("refs/tags/{tag}:refs/tags/{tag}")],
        Tracking::Commit(_) => vec![
            "refs/heads/*:refs/remotes/origin/*".to_string(),
            "refs/tags/*:refs/tags/*".to_string(),
        ],
        Tracking::DefaultBranch => vec!["refs/heads/*:refs/remotes/origin/*".to_string()],
    };
    git::fetch_origin_filtered(repo, &refspecs, CACHE_FILTER).await?;
    if matches!(tracking, Tracking::DefaultBranch) {
        git::set_remote_head(repo, "origin").await?;
    }
    Ok(())
}

/// Clone a local working tree from the persistent cache into staging.
pub async fn materialize_staging(paths: &Paths, plugin_id: &str) -> Result<PathBuf> {
    let cache_dir = paths.repo_cache_dir(plugin_id);
    let staging_dir = paths.staging_dir(plugin_id);
    if staging_dir.exists() {
        std::fs::remove_dir_all(&staging_dir)?;
    }
    git::clone_local_repo(&cache_dir, &staging_dir).await?;
    Ok(staging_dir)
}

/// Resolve a tracking spec against a prepared repository.
pub async fn resolve_tracking(
    repo: &Path,
    tracking: &Tracking,
) -> Result<(String, TrackingRecord)> {
    match tracking {
        Tracking::Branch(branch) => {
            let commit = git::resolve_remote_branch(repo, branch).await?;
            Ok((commit, TrackingRecord { kind: "branch".into(), value: branch.clone() }))
        }
        Tracking::Tag(tag) => {
            let commit = git::resolve_tag(repo, tag).await?;
            Ok((commit, TrackingRecord { kind: "tag".into(), value: tag.clone() }))
        }
        Tracking::Commit(commit) => {
            Ok((commit.clone(), TrackingRecord { kind: "commit".into(), value: commit.clone() }))
        }
        Tracking::DefaultBranch => {
            let branch = git::default_branch(repo).await?;
            let commit = git::resolve_remote_branch(repo, &branch).await?;
            Ok((commit, TrackingRecord { kind: "default-branch".into(), value: branch }))
        }
    }
}

/// Format a human-readable description of how a tracking spec resolved to a commit.
pub fn describe_tracking_resolution(
    tracking: &Tracking,
    record: &TrackingRecord,
    commit: &str,
) -> String {
    let commit = short_hash(commit);
    match tracking {
        Tracking::Tag(tag) => format!("tag@{tag} -> commit@{commit}"),
        Tracking::Branch(branch) => format!("branch@{branch} -> commit@{commit}"),
        Tracking::DefaultBranch => {
            format!("default-branch -> branch@{} -> commit@{commit}", record.value)
        }
        Tracking::Commit(commit) => format!("commit@{}", short_hash(commit)),
    }
}

/// Resolve a tracking spec against the persistent cache without materializing staging.
pub async fn resolve_tracking_revision(
    paths: &Paths,
    plugin_id: &str,
    clone_url: &str,
    tracking: &Tracking,
) -> Result<ResolvedRevision> {
    ensure_cache_repo(paths, plugin_id, clone_url).await?;
    let cache_dir = paths.repo_cache_dir(plugin_id);
    fetch_for_tracking(&cache_dir, tracking).await?;
    let (commit, tracking_record) = resolve_tracking(&cache_dir, tracking).await?;
    Ok(ResolvedRevision { commit, tracking: Some(tracking_record) })
}

/// Ensure a locked commit is available in the persistent cache without staging.
pub async fn ensure_locked_revision(
    paths: &Paths,
    plugin_id: &str,
    clone_url: &str,
    commit: &str,
) -> Result<ResolvedRevision> {
    ensure_cache_repo(paths, plugin_id, clone_url).await?;
    let cache_dir = paths.repo_cache_dir(plugin_id);
    if !git::has_commit(&cache_dir, commit).await? {
        git::fetch_origin_filtered(
            &cache_dir,
            &[
                "refs/heads/*:refs/remotes/origin/*".to_string(),
                "refs/tags/*:refs/tags/*".to_string(),
            ],
            CACHE_FILTER,
        )
        .await?;
    }
    Ok(ResolvedRevision { commit: commit.to_string(), tracking: None })
}

/// Materialize a staging checkout for a resolved revision.
pub async fn materialize_staging_at_revision(
    paths: &Paths,
    plugin_id: &str,
    clone_url: &str,
    revision: &ResolvedRevision,
) -> Result<PreparedRepo> {
    let cache_dir = paths.repo_cache_dir(plugin_id);
    let staging_dir = materialize_staging(paths, plugin_id).await?;
    git::set_remote_url(&staging_dir, "origin", clone_url).await?;
    git::inherit_partial_clone_config(&cache_dir, &staging_dir, "origin").await?;
    if let Err(err) = git::checkout(&staging_dir, &revision.commit).await {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(err);
    }
    Ok(PreparedRepo {
        staging_dir,
        commit: revision.commit.clone(),
        tracking: revision.tracking.clone(),
    })
}
