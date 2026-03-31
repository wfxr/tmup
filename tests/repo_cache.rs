mod utils;

use tempfile::tempdir;
use tmup::git::{self, ObjectFilter};
use utils::*;

#[tokio::test]
async fn clone_bare_repo_and_materialize_staging() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let cache = dir.path().join("cache.git");
    let staging = dir.path().join("staging");
    let clone_url = format!("file://{}", bare.display());

    git::clone_bare_repo(&clone_url, &cache).await.unwrap();
    git::clone_local_repo(&cache, &staging).await.unwrap();

    assert_eq!(git::head_commit(&staging).await.unwrap(), commit);
}

#[tokio::test]
async fn has_commit_reports_cached_revision() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let cache = dir.path().join("cache.git");
    let clone_url = format!("file://{}", bare.display());

    git::clone_bare_repo(&clone_url, &cache).await.unwrap();
    assert!(git::has_commit(&cache, &commit).await.unwrap());
}

#[tokio::test]
async fn fetch_origin_fetches_requested_refspecs() {
    let dir = tempdir().unwrap();
    let (bare, _commit_a) = make_bare_repo(&dir.path().join("repo"));
    let cache = dir.path().join("cache.git");
    let clone_url = format!("file://{}", bare.display());

    git::clone_bare_repo(&clone_url, &cache).await.unwrap();

    let commit_b = push_commit(&bare, "next");
    assert!(!git::has_commit(&cache, &commit_b).await.unwrap());

    let refspecs = vec!["refs/heads/main:refs/remotes/origin/main".to_string()];
    git::fetch_origin(&cache, &refspecs).await.unwrap();

    assert!(git::has_commit(&cache, &commit_b).await.unwrap());
}

#[tokio::test]
async fn clone_bare_repo_filtered_accepts_blob_none_for_local_server() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let cache = dir.path().join("cache.git");
    let clone_url = format!("file://{}", bare.display());

    git::clone_bare_repo_filtered(&clone_url, &cache, Some(ObjectFilter::BlobNone)).await.unwrap();

    assert_eq!(git::head_commit(&cache).await.unwrap(), commit);
}

#[tokio::test]
#[ignore = "requires network access and a remote that supports partial clone filters"]
async fn clone_bare_repo_filtered_marks_promisor_remote_for_live_github_repo() {
    let dir = tempdir().unwrap();
    let cache = dir.path().join("cache.git");
    let clone_url = std::env::var("TMUP_LIVE_PARTIAL_CLONE_REPO")
        .unwrap_or_else(|_| "https://github.com/wfxr/tmux-net-speed.git".to_string());

    git::clone_bare_repo_filtered(&clone_url, &cache, Some(ObjectFilter::BlobNone)).await.unwrap();

    assert_eq!(git(&["config", "--get", "remote.origin.promisor"], &cache), "true");
    assert_eq!(git(&["config", "--get", "remote.origin.partialclonefilter"], &cache), "blob:none");
}
