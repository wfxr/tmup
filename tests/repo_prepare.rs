mod utils;

use std::fs;
use std::path::Path;
use std::process::Command;

use lazytmux::model::Tracking;
use lazytmux::state::Paths;
use lazytmux::{git, repo};
use tempfile::tempdir;
use utils::*;

async fn prepare_tracking(
    paths: &Paths,
    plugin_id: &str,
    clone_url: &str,
    tracking: &Tracking,
) -> repo::PreparedRepo {
    let revision =
        repo::resolve_tracking_revision(paths, plugin_id, clone_url, tracking).await.unwrap();
    repo::materialize_staging_at_revision(paths, plugin_id, clone_url, &revision).await.unwrap()
}

fn git_bare(args: &[&str], git_dir: &Path) -> String {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} {:?} failed: {}",
        args,
        git_dir,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn remove_blob_from_cache_and_mark_promisor(cache_dir: &Path) {
    let tree = git_bare(&["ls-tree", "-r", "HEAD"], cache_dir);
    let blob = tree
        .lines()
        .find_map(|line| line.split_whitespace().nth(2))
        .expect("cache repo should contain at least one blob")
        .to_string();
    let (fanout, rest) = blob.split_at(2);
    std::fs::remove_file(cache_dir.join("objects").join(fanout).join(rest)).unwrap();
    std::fs::write(cache_dir.join("objects/pack/synthetic.promisor"), "").unwrap();

    git_bare(&["config", "extensions.partialClone", "origin"], cache_dir);
    git_bare(&["config", "remote.origin.promisor", "true"], cache_dir);
    git_bare(&["config", "remote.origin.partialclonefilter", "blob:none"], cache_dir);
}

fn synthesize_promisor_cache_from_worktree(worktree: &Path, cache_dir: &Path, clone_url: &str) {
    fs::create_dir_all(cache_dir.parent().unwrap()).unwrap();
    git(&["init", "--bare", cache_dir.to_str().unwrap()], cache_dir.parent().unwrap());

    let source_objects = worktree.join(".git/objects");
    copy_loose_objects(&source_objects, &cache_dir.join("objects"));

    let commit = git(&["rev-parse", "HEAD"], worktree);
    fs::write(cache_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::create_dir_all(cache_dir.join("refs/heads")).unwrap();
    fs::write(cache_dir.join("refs/heads/main"), format!("{commit}\n")).unwrap();
    git_bare(&["config", "remote.origin.url", clone_url], cache_dir);
}

fn copy_loose_objects(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "info" || name == "pack" {
            continue;
        }

        let src = entry.path();
        let dst = to.join(name.as_ref());
        if file_type.is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_loose_objects(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

#[tokio::test]
async fn prepare_tracking_staging_creates_cache_and_checks_out_latest_commit() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());

    let prepared =
        prepare_tracking(&paths, "example.com/test/plugin", &clone_url, &Tracking::DefaultBranch)
            .await;

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

    let prepared =
        prepare_tracking(&paths, "example.com/test/plugin", &clone_url, &Tracking::DefaultBranch)
            .await;

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

    let first =
        prepare_tracking(&paths, "example.com/test/plugin", &clone_url_a, &Tracking::DefaultBranch)
            .await;
    assert_eq!(git::head_commit(&first.staging_dir).await.unwrap(), commit_a);
    std::fs::remove_dir_all(&first.staging_dir).unwrap();

    let second =
        prepare_tracking(&paths, "example.com/test/plugin", &clone_url_b, &Tracking::DefaultBranch)
            .await;

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

    let prepared = prepare_tracking(&paths, plugin_id, &clone_url, &Tracking::DefaultBranch).await;

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

    let prepared =
        prepare_tracking(&paths, "example.com/test/plugin", &clone_url, &Tracking::DefaultBranch)
            .await;

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

#[tokio::test]
async fn materialize_staging_at_revision_recovers_missing_blobs_from_origin() {
    let dir = tempdir().unwrap();
    let repo_root = dir.path().join("repo");
    let (bare, commit) = make_bare_repo(&repo_root);
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();
    let clone_url = format!("file://{}", bare.display());
    let plugin_id = "example.com/test/plugin";

    let cache_dir = paths.repo_cache_dir(plugin_id);
    synthesize_promisor_cache_from_worktree(&repo_root.join("work"), &cache_dir, &clone_url);
    remove_blob_from_cache_and_mark_promisor(&cache_dir);

    let revision =
        repo::resolve_tracking_revision(&paths, plugin_id, &clone_url, &Tracking::DefaultBranch)
            .await
            .unwrap();

    let prepared =
        repo::materialize_staging_at_revision(&paths, plugin_id, &clone_url, &revision).await;

    let prepared = prepared.expect("staging checkout should recover missing blobs from origin");
    assert_eq!(git::head_commit(&prepared.staging_dir).await.unwrap(), commit);
    assert_eq!(
        std::fs::read_to_string(prepared.staging_dir.join("init.tmux")).unwrap(),
        "#!/bin/sh\n"
    );
}
