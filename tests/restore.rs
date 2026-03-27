mod utils;
use lazytmux::lockfile::{LockEntry, LockFile, TrackingRecord};
use lazytmux::model::{Config, Options, PluginSource, PluginSpec, Tracking};
use lazytmux::plugin;
use lazytmux::progress::NullReporter;
use lazytmux::state::Paths;
use tempfile::tempdir;
use utils::*;

/// Reset the bare repo's main branch to a given commit.
fn reset_bare(bare: &std::path::Path, commit: &str) {
    git(&["update-ref", "refs/heads/main", commit], bare);
}

/// Build a Config with a single remote plugin pointing at a local bare repo.
fn make_config(clone_url: &str, build: Option<&str>) -> Config {
    make_config_with_tracking(clone_url, Tracking::DefaultBranch, build)
}

fn make_config_with_tracking(clone_url: &str, tracking: Tracking, build: Option<&str>) -> Config {
    Config {
        options: Options::default(),
        plugins: vec![PluginSpec {
            source: PluginSource::Remote {
                raw: "test/plugin".into(),
                id: "example.com/test/plugin".into(),
                clone_url: clone_url.into(),
            },
            name: "plugin".into(),
            opt_prefix: String::new(),
            tracking,
            build: build.map(String::from),
            opts: vec![],
        }],
    }
}

// ---------------------------------------------------------------------------
// Regression: same-commit restore must not replace build artifacts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_same_commit_preserves_build_artifacts() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, Some("touch built.marker"));

    let mut lock = LockFile::new();
    lock.plugins.insert(
        "example.com/test/plugin".into(),
        LockEntry {
            tracking: TrackingRecord { kind: "branch".into(), value: "main".into() },
            commit: commit.clone(),
            config_hash: None,
        },
    );

    // First restore: installs from scratch, build runs and creates marker.
    plugin::restore(&cfg, &lock, &paths, None, &NullReporter).await.unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(target.join("built.marker").exists(), "build should have created marker");

    // Second restore: same commit — must be a no-op.
    plugin::restore(&cfg, &lock, &paths, None, &NullReporter).await.unwrap();
    assert!(
        target.join("built.marker").exists(),
        "same-commit restore must not replace the directory and lose build artifacts"
    );
}

#[tokio::test]
async fn restore_errors_when_locked_commit_is_missing_from_cache_and_remote_disappears() {
    let dir = tempdir().unwrap();
    let (bare, commit_a) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, None);

    let mut lock = LockFile::new();
    lock.plugins.insert(
        "example.com/test/plugin".into(),
        LockEntry {
            tracking: TrackingRecord { kind: "branch".into(), value: "main".into() },
            commit: commit_a.clone(),
            config_hash: None,
        },
    );

    // Seed the cache and installation with commit_a.
    plugin::restore(&cfg, &lock, &paths, None, &NullReporter).await.unwrap();
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(paths.repo_cache_dir("example.com/test/plugin").exists());

    // Advance the remote after the cache has already been populated so the new
    // commit is not present locally yet.
    let commit_b = push_commit(&bare, "second");

    // Advance the desired restore target to a commit that has never been fetched
    // into cache, then remove both the installed checkout and the remote so the
    // second restore cannot recover it.
    lock.plugins.get_mut("example.com/test/plugin").unwrap().commit = commit_b;
    std::fs::remove_dir_all(&target).unwrap();
    std::fs::remove_dir_all(&bare).unwrap();

    let result = plugin::restore(&cfg, &lock, &paths, None, &NullReporter).await;
    assert!(
        result.is_err(),
        "restore should error when the locked commit is not cached and fetch fails"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("git fetch origin failed")
            || err_msg.contains("No such file or directory"),
        "error should mention fetch failure when remote disappears: {err_msg}"
    );
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(!target.exists(), "failed restore must not leave a partial plugin directory");
}

// ---------------------------------------------------------------------------
// Regression: restore build failure must return Err (non-zero exit)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_build_failure_returns_error() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    // Build command that always fails.
    let cfg = make_config(&clone_url, Some("exit 1"));

    let mut lock = LockFile::new();
    lock.plugins.insert(
        "example.com/test/plugin".into(),
        LockEntry {
            tracking: TrackingRecord { kind: "branch".into(), value: "main".into() },
            commit: commit.clone(),
            config_hash: None,
        },
    );

    let result = plugin::restore(&cfg, &lock, &paths, None, &NullReporter).await;
    assert!(result.is_err(), "restore must propagate build failure as Err");

    // The target should have been rolled back / removed by publish protocol.
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(!target.exists(), "failed fresh-install target should be cleaned up");

    // A failure marker should have been written.
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].plugin_id, "example.com/test/plugin");
}

#[tokio::test]
async fn install_build_failure_leaves_no_target_or_lock_entry() {
    let dir = tempdir().unwrap();
    let (bare, _commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, Some("exit 1"));
    let mut lock = LockFile::new();

    let result = plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await;
    assert!(result.is_err(), "install must propagate build failure as Err");

    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(!target.exists(), "fresh install failure should not leave a target directory");
    assert!(
        !lock.plugins.contains_key("example.com/test/plugin"),
        "failed install must not write a lock entry for the plugin"
    );
}

// ---------------------------------------------------------------------------
// Regression: failed restore → same-commit restore clears markers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_same_commit_noop_clears_failure_markers() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());

    // Step 1: install at commit with a succeeding build.
    let cfg_ok = make_config(&clone_url, Some("touch built.marker"));
    let mut lock = LockFile::new();
    plugin::install(&cfg_ok, &mut lock, &paths, None, false, &NullReporter).await.unwrap();
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(target.exists());

    // Step 2: write a failure marker manually (simulating a prior failed operation).
    let marker = lazytmux::state::FailureMarker {
        plugin_id: "example.com/test/plugin".into(),
        commit: commit.clone(),
        build_hash: "fakehash".into(),
        build_command: "exit 1".into(),
        failed_at: "2024-01-01T00:00:00Z".into(),
        stderr_summary: String::new(),
    };
    lazytmux::state::write_failure_marker(&paths.failures_root, &marker).unwrap();
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert_eq!(markers.len(), 1, "failure marker should be present");

    // Step 3: restore — disk HEAD already equals lock commit, so this is a no-op.
    // It should still clear the stale failure marker.
    plugin::restore(&cfg_ok, &lock, &paths, None, &NullReporter).await.unwrap();

    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert!(
        markers.is_empty(),
        "failure markers should be cleared after same-commit restore no-op"
    );
}

// ---------------------------------------------------------------------------
// Regression: failed update → remote rollback → same-commit update clears markers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_same_commit_noop_clears_failure_markers() {
    let dir = tempdir().unwrap();
    let (bare, commit_a) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    // Build that always fails — used to produce a failure marker.
    let cfg_fail = make_config(&clone_url, Some("exit 1"));

    // Step 1: install at commit_a with a succeeding build, so the plugin is on disk.
    let cfg_ok = make_config(&clone_url, Some("touch built.marker"));
    let mut lock = LockFile::new();
    plugin::install(&cfg_ok, &mut lock, &paths, None, false, &NullReporter).await.unwrap();
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(target.exists());
    assert_eq!(git(&["rev-parse", "HEAD"], &target), commit_a);
    assert!(target.join("built.marker").exists(), "successful install should leave marker");

    // Step 2: push a new commit (commit_b) and attempt update with failing build.
    let commit_b = push_commit(&bare, "second");
    assert_ne!(commit_a, commit_b);

    let result = plugin::update(&cfg_fail, &mut lock, &paths, None, &NullReporter).await;
    assert!(result.is_err(), "update with failing build should error");
    assert_eq!(
        git(&["rev-parse", "HEAD"], &target),
        commit_a,
        "failed staged build must leave the previously installed commit untouched"
    );
    assert!(
        target.join("built.marker").exists(),
        "failed staged build must not replace the existing target directory"
    );
    assert_eq!(
        lock.plugins["example.com/test/plugin"].commit, commit_a,
        "failed update must not advance the in-memory lock entry"
    );

    // Failure marker should exist.
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert!(!markers.is_empty(), "failure marker should be recorded");

    // Step 3: remote resets main back to commit_a (simulating upstream rollback).
    reset_bare(&bare, &commit_a);

    // Step 4: update again — remote now resolves to commit_a which is already
    // installed, so this is a same-commit no-op. It should succeed AND clear markers.
    let cfg_ok2 = make_config(&clone_url, Some("touch built.marker"));
    let result = plugin::update(&cfg_ok2, &mut lock, &paths, None, &NullReporter).await;
    assert!(result.is_ok(), "same-commit update should succeed");

    // Failure markers should now be cleared.
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert!(
        markers.is_empty(),
        "failure markers should be cleared after successful same-commit update"
    );
}

// ---------------------------------------------------------------------------
// Regression: healthy + no lock should still run the full install path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_reinstalls_healthy_branch_repo_without_lock() {
    let dir = tempdir().unwrap();
    let (bare, _commit_a) = make_bare_repo(&dir.path().join("repo"));
    let commit_b = push_commit(&bare, "second");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    clone_to_target(&bare, &target);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), commit_b);

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config_with_tracking(
        &clone_url,
        Tracking::Branch("main".into()),
        Some("touch built.marker"),
    );

    let mut lock = LockFile::new();
    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, commit_b);
    assert_eq!(entry.tracking.kind, "branch");
    assert_eq!(entry.tracking.value, "main");
    assert_eq!(git(&["rev-parse", "HEAD"], &target), entry.commit);
    assert!(
        target.join("built.marker").exists(),
        "install should run the build even when the repo is already present"
    );
}

#[tokio::test]
async fn install_repairs_healthy_repo_when_branch_head_is_on_other_branch() {
    let dir = tempdir().unwrap();
    let (bare, main_commit) = make_bare_repo(&dir.path().join("repo"));
    let feature_commit = push_branch_commit(&bare, "feature", "feature");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    clone_to_target(&bare, &target);
    git(&["checkout", &feature_commit], &target);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), feature_commit);

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config_with_tracking(&clone_url, Tracking::Branch("main".into()), None);

    let mut lock = LockFile::new();
    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, main_commit);
    assert_eq!(entry.tracking.kind, "branch");
    assert_eq!(entry.tracking.value, "main");
    assert_eq!(git(&["rev-parse", "HEAD"], &target), main_commit);
}

#[tokio::test]
async fn install_repairs_healthy_repo_when_default_branch_head_is_on_other_branch() {
    let dir = tempdir().unwrap();
    let (bare, main_commit) = make_bare_repo(&dir.path().join("repo"));
    let feature_commit = push_branch_commit(&bare, "feature", "feature");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    clone_to_target(&bare, &target);
    git(&["checkout", &feature_commit], &target);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), feature_commit);

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, None);

    let mut lock = LockFile::new();
    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, main_commit);
    assert_eq!(entry.tracking.kind, "default-branch");
    assert_eq!(entry.tracking.value, "main");
    assert_eq!(git(&["rev-parse", "HEAD"], &target), main_commit);
}

// ---------------------------------------------------------------------------
// Regression: healthy + no lock + pinned commit must replace mismatched repo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_repairs_healthy_repo_when_pinned_commit_differs() {
    let dir = tempdir().unwrap();
    let (bare, commit_a) = make_bare_repo(&dir.path().join("repo"));
    let commit_b = push_commit(&bare, "second");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    clone_to_target(&bare, &target);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), commit_b);

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config_with_tracking(&clone_url, Tracking::Commit(commit_a.clone()), None);

    let mut lock = LockFile::new();
    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, commit_a);
    assert_eq!(entry.tracking.kind, "commit");
    assert_eq!(entry.tracking.value, entry.commit);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), entry.commit);
}

// ---------------------------------------------------------------------------
// Regression: healthy + no lock + pinned tag must replace mismatched repo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn install_repairs_healthy_repo_when_pinned_tag_differs() {
    let dir = tempdir().unwrap();
    let (bare, commit_a) = make_bare_repo(&dir.path().join("repo"));
    push_tag(&bare, "v1.0.0", &commit_a);
    let commit_b = push_commit(&bare, "second");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    clone_to_target(&bare, &target);
    assert_eq!(git(&["rev-parse", "HEAD"], &target), commit_b);

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config_with_tracking(&clone_url, Tracking::Tag("v1.0.0".into()), None);

    let mut lock = LockFile::new();
    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, commit_a);
    assert_eq!(entry.tracking.kind, "tag");
    assert_eq!(entry.tracking.value, "v1.0.0");
    assert_eq!(git(&["rev-parse", "HEAD"], &target), entry.commit);
}

#[tokio::test]
async fn install_build_runs_in_staging_before_swap() {
    let dir = tempdir().unwrap();
    let (bare, _commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, Some("pwd > .build-cwd"));
    let mut lock = LockFile::new();

    plugin::install(&cfg, &mut lock, &paths, None, false, &NullReporter).await.unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    let cwd = std::fs::read_to_string(target.join(".build-cwd")).unwrap();

    assert!(cwd.contains(".staging"), "Build should run inside staging: {cwd}");
    assert!(!cwd.contains("/plugins/"), "Staging path should be distinct from plugin root: {cwd}");
}
