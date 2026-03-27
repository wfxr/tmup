mod utils;

use lazytmux::model::Tracking;
use lazytmux::state::Paths;
use lazytmux::{git, repo};
use tempfile::tempdir;
use utils::*;

#[tokio::test]
async fn prepare_tracking_staging_creates_cache_and_checks_out_latest_commit() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let prepared = repo::prepare_tracking_staging(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    assert_eq!(git::head_commit(&prepared.staging_dir).await.unwrap(), commit);
    assert!(paths.repo_cache_dir("example.com/test/plugin").exists());
}

#[tokio::test]
async fn prepare_tracking_handles_non_main_default_branch() {
    let dir = tempdir().unwrap();
    let work = dir.path().join("trunk-work");
    std::fs::create_dir_all(&work).unwrap();
    git(&["init", "-b", "trunk"], &work);
    std::fs::write(work.join("init.tmux"), "#!/bin/sh\n").unwrap();
    git(&["add", "."], &work);
    git(&["commit", "-m", "init"], &work);
    let commit = git(&["rev-parse", "HEAD"], &work);

    let bare = dir.path().join("bare.git");
    git(&["clone", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()], dir.path());

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let prepared = repo::prepare_tracking_staging(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    assert_eq!(git::head_commit(&prepared.staging_dir).await.unwrap(), commit);
    assert!(paths.repo_cache_dir("example.com/test/plugin").exists());
}

#[tokio::test]
async fn prepare_tracking_refreshes_cache_origin_when_clone_url_changes() {
    let dir = tempdir().unwrap();
    let (bare_a, commit_a) = make_bare_repo(&dir.path().join("repo-a"));
    let (bare_b, _initial_b) = make_bare_repo(&dir.path().join("repo-b"));
    let commit_b = push_commit(&bare_b, "new-remote");
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url_a = format!("file://{}", bare_a.display());
    let clone_url_b = format!("file://{}", bare_b.display());

    let first = repo::prepare_tracking_staging(
        &paths,
        "example.com/test/plugin",
        &clone_url_a,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();
    assert_eq!(git::head_commit(&first.staging_dir).await.unwrap(), commit_a);
    std::fs::remove_dir_all(&first.staging_dir).unwrap();

    let second = repo::prepare_tracking_staging(
        &paths,
        "example.com/test/plugin",
        &clone_url_b,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    assert_eq!(git::head_commit(&second.staging_dir).await.unwrap(), commit_b);
    assert_eq!(
        git(&["remote", "get-url", "origin"], &paths.repo_cache_dir("example.com/test/plugin")),
        clone_url_b
    );
}

#[tokio::test]
async fn prepare_tracking_cleans_stale_staging_dir_before_clone() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());
    let plugin_id = "example.com/test/plugin";
    let stale_staging = paths.staging_dir(plugin_id);

    std::fs::create_dir_all(&stale_staging).unwrap();
    std::fs::write(stale_staging.join("partial-clone.txt"), "stale").unwrap();

    let prepared =
        repo::prepare_tracking_staging(&paths, plugin_id, &clone_url, &Tracking::DefaultBranch)
            .await
            .unwrap();

    assert_eq!(git::head_commit(&prepared.staging_dir).await.unwrap(), commit);
    assert!(!prepared.staging_dir.join("partial-clone.txt").exists());
}

#[tokio::test]
async fn prepare_tracking_ignores_remote_head_update_failures() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    std::fs::write(bare.join("HEAD"), "ref: refs/heads/missing\n").unwrap();

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let prepared = repo::prepare_tracking_staging(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    assert_eq!(git::head_commit(&prepared.staging_dir).await.unwrap(), commit);
}

#[tokio::test]
async fn resolve_tracking_revision_exposes_commit_without_staging() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let revision = repo::resolve_tracking_revision(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    assert_eq!(revision.commit, commit);
    assert_eq!(revision.tracking.as_ref().unwrap().kind, "default-branch");
}

#[tokio::test]
async fn materialize_staging_at_revision_reuses_staging_and_cleans_old() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let revision = repo::resolve_tracking_revision(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &Tracking::DefaultBranch,
    )
    .await
    .unwrap();

    let first = repo::materialize_staging_at_revision(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &revision,
    )
    .await
    .unwrap();
    std::fs::write(first.staging_dir.join("marker"), "stale").unwrap();

    let second = repo::materialize_staging_at_revision(
        &paths,
        "example.com/test/plugin",
        &clone_url,
        &revision,
    )
    .await
    .unwrap();

    assert_eq!(git::head_commit(&second.staging_dir).await.unwrap(), commit);
    assert!(!second.staging_dir.join("marker").exists(), "stale staging content should be cleared");
}
