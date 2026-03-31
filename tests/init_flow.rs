mod utils;

use tempfile::tempdir;
use tmup::config::parse_config;
use tmup::lockfile::{
    LockEntry, LockFile, config_fingerprint, read_lockfile, remote_plugin_config_hash,
};
use tmup::model::{Config, Options, PluginSource, PluginSpec, Tracking};
use tmup::progress::NullReporter;
use tmup::state::{OperationLock, Paths, build_command_hash};
use tmup::sync;
use utils::*;

fn make_plugin(clone_url: &str, tracking: Tracking, build: Option<&str>) -> PluginSpec {
    PluginSpec {
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
    }
}

fn make_config_from_plugin(plugin: PluginSpec) -> Config {
    Config { options: Options::default(), plugins: vec![plugin] }
}

#[test]
fn init_preview_returns_false_when_aligned() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let plugin_dir = paths.plugin_dir("github.com/user/repo");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    git(&["init", "-b", "main"], &plugin_dir);
    std::fs::write(plugin_dir.join("init.tmux"), "#!/bin/sh\n").unwrap();
    git(&["add", "."], &plugin_dir);
    git(&["commit", "-m", "init"], &plugin_dir);
    let commit = git(&["rev-parse", "HEAD"], &plugin_dir);

    let config = parse_config(r#"plugin "user/repo""#).unwrap();
    let mut lock = LockFile::new();
    let mut entry = LockEntry::branch("main", &commit);
    entry.config_hash = remote_plugin_config_hash(&config.plugins[0]);
    lock.plugins.insert("github.com/user/repo".into(), entry);

    let preview = sync::preview(&config, &lock, None, sync::SyncPolicy::init(true), &paths);
    assert!(!preview.needs_work);
}

#[test]
fn init_preview_returns_true_for_missing_plugin_dir_under_init_policy() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let config = parse_config(r#"plugin "user/repo""#).unwrap();
    let mut lock = LockFile::new();
    let mut entry = LockEntry::branch("main", "abc123");
    entry.config_hash = remote_plugin_config_hash(&config.plugins[0]);
    lock.plugins.insert("github.com/user/repo".into(), entry);

    assert!(!paths.plugin_dir("github.com/user/repo").exists());
    let preview = sync::preview(&config, &lock, None, sync::SyncPolicy::init(true), &paths);
    assert!(
        preview.needs_work,
        "preview should require work when plugin dir is missing even if lock hash matches"
    );
}

#[test]
fn init_preview_returns_true_for_broken_plugin_dir_under_init_policy() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let config = parse_config(r#"plugin "user/repo""#).unwrap();
    let mut lock = LockFile::new();
    let mut entry = LockEntry::branch("main", "abc123");
    entry.config_hash = remote_plugin_config_hash(&config.plugins[0]);
    lock.plugins.insert("github.com/user/repo".into(), entry);

    let plugin_dir = paths.plugin_dir("github.com/user/repo");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    assert!(plugin_dir.exists());
    assert!(!plugin_dir.join(".git").exists());

    let preview = sync::preview(&config, &lock, None, sync::SyncPolicy::init(true), &paths);
    assert!(
        preview.needs_work,
        "preview should require work when plugin dir is broken even if lock hash matches"
    );
}

#[test]
fn init_preview_returns_true_for_same_commit_build_change_requires_republish() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    let (old_cfg, new_cfg) = (
        parse_config(r#"plugin "user/repo" build="touch built-v1""#).unwrap(),
        parse_config(r#"plugin "user/repo" build="touch built-v2""#).unwrap(),
    );
    let mut lock = LockFile::new();
    let mut entry = LockEntry::branch("main", "abc123");
    entry.config_hash = remote_plugin_config_hash(&old_cfg.plugins[0]);
    lock.plugins.insert("github.com/user/repo".into(), entry);

    let preview = sync::preview(&new_cfg, &lock, None, sync::SyncPolicy::init(true), &paths);
    assert!(
        preview.needs_work,
        "preview should require work when same-commit build config changes"
    );
}

#[tokio::test]
async fn init_does_not_retry_same_failed_build_tuple() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let marker_path = dir.path().join("build-retried.marker");
    let build_cmd = format!(": > \"{}\"; exit 1", marker_path.display());
    let plugin = make_plugin(&clone_url, Tracking::DefaultBranch, Some(&build_cmd));
    let old_plugin = make_plugin(&clone_url, Tracking::DefaultBranch, Some("touch old.marker"));
    let cfg = make_config_from_plugin(plugin);

    let mut lock = LockFile::new();
    let mut entry = LockEntry::default_branch("main", &commit);
    entry.config_hash = remote_plugin_config_hash(&old_plugin);
    lock.plugins.insert("example.com/test/plugin".into(), entry);

    let bh = build_command_hash(&build_cmd);
    let marker = tmup::state::FailureMarker {
        plugin_id: "example.com/test/plugin".into(),
        commit: commit.clone(),
        build_hash: bh.clone(),
        build_command: build_cmd.clone(),
        failed_at: "now".into(),
        stderr_summary: "error".into(),
    };
    tmup::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    let outcome = sync::run_and_write(
        &cfg,
        &mut lock,
        &paths,
        None,
        sync::SyncPolicy::init(true),
        sync::SyncMode::Init,
        &NullReporter,
    )
    .await
    .unwrap();

    assert!(
        outcome.plugin_failures.is_empty(),
        "init-mode sync should suppress known failed (id, commit, build) tuples"
    );
    assert!(
        !marker_path.exists(),
        "init-mode sync should skip publish/build when tuple is already known-failed"
    );
}

#[tokio::test]
async fn init_retries_when_build_command_changes() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let marker_path = dir.path().join("build-retried.marker");
    let previous_build = "make install";
    let new_build = format!(": > \"{}\"; exit 1", marker_path.display());
    let plugin = make_plugin(&clone_url, Tracking::DefaultBranch, Some(&new_build));
    let old_plugin = make_plugin(&clone_url, Tracking::DefaultBranch, Some(previous_build));
    let cfg = make_config_from_plugin(plugin);

    let mut lock = LockFile::new();
    let mut entry = LockEntry::default_branch("main", &commit);
    entry.config_hash = remote_plugin_config_hash(&old_plugin);
    lock.plugins.insert("example.com/test/plugin".into(), entry);

    let marker = tmup::state::FailureMarker {
        plugin_id: "example.com/test/plugin".into(),
        commit: commit.clone(),
        build_hash: build_command_hash(previous_build),
        build_command: previous_build.into(),
        failed_at: "now".into(),
        stderr_summary: "error".into(),
    };
    tmup::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    let outcome = sync::run_and_write(
        &cfg,
        &mut lock,
        &paths,
        None,
        sync::SyncPolicy::init(true),
        sync::SyncMode::Init,
        &NullReporter,
    )
    .await
    .unwrap();

    assert_eq!(
        outcome.plugin_failures.len(),
        1,
        "changed build command should not be suppressed and should retry"
    );
    assert!(
        marker_path.exists(),
        "changed build command should execute build and touch retry marker"
    );
}

#[test]
fn operation_lock_blocks_concurrent_init() {
    let dir = tempdir().unwrap();
    let lock_path = dir.path().join("operations.lock");

    // First process holds the lock
    let _guard = OperationLock::try_acquire(&lock_path).unwrap().expect("should acquire");

    // Second process cannot acquire
    assert!(OperationLock::try_acquire(&lock_path).unwrap().is_none());
}

#[tokio::test]
async fn init_preflight_sync_failure_preserves_previous_lock_snapshot() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let old_plugin = make_plugin(&clone_url, Tracking::DefaultBranch, Some("touch built-v1"));
    let new_plugin =
        make_plugin(&clone_url, Tracking::DefaultBranch, Some("touch built-v2; exit 1"));

    let mut lock = LockFile::new();
    let mut entry = LockEntry::default_branch("main", &commit);
    entry.config_hash = remote_plugin_config_hash(&old_plugin);
    lock.plugins.insert("example.com/test/plugin".into(), entry);
    lock.config_fingerprint = Some(config_fingerprint(&make_config_from_plugin(old_plugin)));

    let cfg = make_config_from_plugin(new_plugin);
    let result = sync::run_and_write(
        &cfg,
        &mut lock,
        &paths,
        None,
        sync::SyncPolicy::init(true),
        sync::SyncMode::Init,
        &NullReporter,
    )
    .await;
    let outcome = result.expect("init sync should surface plugin build failures in SyncOutcome");
    assert_eq!(outcome.plugin_failures.len(), 1, "expected one plugin-level sync failure");
    assert!(
        outcome.plugin_failures[0].contains("example.com/test/plugin"),
        "plugin failure should include plugin id"
    );

    let persisted = read_lockfile(&paths.lockfile_path).unwrap();
    let entry = persisted.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, commit);
    assert_eq!(entry.tracking.kind, "default-branch");
    assert_eq!(entry.config_hash, lock.plugins["example.com/test/plugin"].config_hash);
}
