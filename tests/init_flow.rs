use std::collections::{HashMap, HashSet};

use lazytmux::{
    config::parse_config,
    lockfile::{LockEntry, LockFile},
    planner,
    planner::RepoHealth,
    state::{OperationLock, Paths, build_command_hash},
};
use tempfile::tempdir;

#[test]
fn init_read_only_path_detected_when_aligned() {
    let config = parse_config(r#"plugin "user/repo""#).unwrap();
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy {
            commit: "abc123".into(),
        })]
        .into();

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
    assert!(
        plan.to_install
            .contains(&"github.com/user/repo".to_string())
    );
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
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    // Between preflight and lock acquisition, plugin was installed
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy {
            commit: "abc123".into(),
        })]
        .into();

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
        plugin_id:      "github.com/user/repo".into(),
        commit:         "abc123".into(),
        build_hash:     bh.clone(),
        build_command:  "make install".into(),
        failed_at:      "now".into(),
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
        plugin_id:      "github.com/user/repo".into(),
        commit:         "abc123".into(),
        build_hash:     build_command_hash("make install"),
        build_command:  "make install".into(),
        failed_at:      "now".into(),
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
    let _guard = OperationLock::try_acquire(&lock_path)
        .unwrap()
        .expect("should acquire");

    // Second process detects writer active
    assert!(OperationLock::is_writer_active(&lock_path).unwrap());

    // Second process cannot acquire
    assert!(OperationLock::try_acquire(&lock_path).unwrap().is_none());
}
