mod utils;
use std::collections::{HashMap, HashSet};

use lazytmux::config::parse_config;
use lazytmux::lockfile::{
    LockEntry, LockFile, config_fingerprint, read_lockfile, remote_plugin_config_hash,
};
use lazytmux::model::{Config, Options, PluginSource, PluginSpec, Tracking};
use lazytmux::planner::RepoHealth;
use lazytmux::state::{OperationLock, Paths, build_command_hash};
use lazytmux::{planner, sync};
use tempfile::tempdir;
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
fn init_read_only_path_detected_when_aligned() {
    let config = parse_config(r#"plugin "user/repo""#).unwrap();
    let mut lock = LockFile::new();
    lock.plugins
        .insert("github.com/user/repo".into(), LockEntry::branch("user/repo", "main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();

    let plan = planner::plan_init(&config, &lock, &health, &HashSet::new());
    assert!(plan.is_none());
}

#[test]
fn init_write_plan_when_plugin_missing_and_auto_install() {
    let config = parse_config(
        r#"
options { auto-install #true }
plugin "user/repo"
    "#,
    )
    .unwrap();
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> = HashMap::new();

    let plan = planner::plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected Some(WritePlan)");
    assert!(plan.to_install.contains(&"github.com/user/repo".to_string()));
}

#[test]
fn init_replans_inside_lock_before_mutation() {
    // Simulates: preflight says "need install", but by the time we get the lock,
    // another process already installed it. Replan should detect no writes needed.
    let config = parse_config(
        r#"
options { auto-install #true }
plugin "user/repo"
    "#,
    )
    .unwrap();
    let mut lock = LockFile::new();
    lock.plugins
        .insert("github.com/user/repo".into(), LockEntry::branch("user/repo", "main", "abc123"));
    // Between preflight and lock acquisition, plugin was installed
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();

    let plan = planner::plan_init(&config, &lock, &health, &HashSet::new());
    assert!(plan.is_none());
}

#[test]
fn init_does_not_retry_same_failed_build_tuple() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    // Write a failure marker
    let bh = build_command_hash("make install");
    let marker = lazytmux::state::FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: bh.clone(),
        build_command: "make install".into(),
        failed_at: "now".into(),
        stderr_summary: "error".into(),
    };
    lazytmux::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    // Same (id, commit, build hash) should be detected as known failure
    assert!(
        lazytmux::plugin::is_known_failure(
            &paths,
            "github.com/user/repo",
            "abc123",
            "make install"
        )
        .unwrap()
    );
}

#[test]
fn init_retries_when_build_command_changes() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    // Write a failure marker for "make install"
    let marker = lazytmux::state::FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: build_command_hash("make install"),
        build_command: "make install".into(),
        failed_at: "now".into(),
        stderr_summary: "error".into(),
    };
    lazytmux::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    // Changed build command: should NOT be known failure
    assert!(
        !lazytmux::plugin::is_known_failure(&paths, "github.com/user/repo", "abc123", "just build")
            .unwrap()
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
    let mut entry = LockEntry::default_branch("test/plugin", "main", &commit);
    entry.config_hash = remote_plugin_config_hash(&old_plugin);
    lock.plugins.insert("example.com/test/plugin".into(), entry);
    lock.config_fingerprint = Some(config_fingerprint(&make_config_from_plugin(old_plugin)));

    let cfg = make_config_from_plugin(new_plugin);
    let result =
        sync::run_and_write(&cfg, &mut lock, &paths, None, sync::SyncPolicy::init(true)).await;
    assert!(result.is_err(), "init preflight should abort on sync failure");

    let persisted = read_lockfile(&paths.lockfile_path).unwrap();
    let entry = persisted.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.commit, commit);
    assert_eq!(entry.tracking.kind, "default-branch");
    assert_eq!(entry.config_hash, lock.plugins["example.com/test/plugin"].config_hash);
}
