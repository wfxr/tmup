use tempfile::tempdir;
use tmup::state::{
    FailureKey, FailureMarker, OperationLock, Paths, build_command_hash, clear_failure_markers,
    has_failure_marker, read_failure_markers, write_failure_marker,
};

#[test]
fn paths_keep_plugins_and_staging_on_same_data_root() {
    let paths = Paths::for_test("/tmp/data", "/tmp/state");
    assert_eq!(paths.plugin_root.parent().unwrap(), paths.staging_root.parent().unwrap());
}

#[test]
fn failure_key_changes_when_build_command_changes() {
    let a = FailureKey::new("github.com/user/repo", "abc123", &build_command_hash("make install"));
    let b = FailureKey::new("github.com/user/repo", "abc123", &build_command_hash("just build"));
    assert_ne!(a, b);
}

#[test]
fn failure_key_changes_when_commit_changes() {
    let hash = build_command_hash("make install");
    let a = FailureKey::new("github.com/user/repo", "abc123", &hash);
    let b = FailureKey::new("github.com/user/repo", "def456", &hash);
    assert_ne!(a, b);
}

#[test]
fn failure_key_same_when_all_components_match() {
    let hash = build_command_hash("make install");
    let a = FailureKey::new("github.com/user/repo", "abc123", &hash);
    let b = FailureKey::new("github.com/user/repo", "abc123", &hash);
    assert_eq!(a, b);
}

#[test]
fn failure_marker_round_trip() {
    let dir = tempdir().unwrap();
    let failures_root = dir.path().join("failures");

    let marker = FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: build_command_hash("make install"),
        build_command: "make install".into(),
        failed_at: "2025-01-01T00:00:00Z".into(),
        stderr_summary: "error: something failed".into(),
    };

    write_failure_marker(&failures_root, &marker).unwrap();

    let key = marker.key();
    assert!(has_failure_marker(&failures_root, &key).unwrap());

    let markers = read_failure_markers(&failures_root).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].plugin_id, "github.com/user/repo");
}

#[test]
fn paths_keep_repo_cache_on_same_data_root() {
    let paths = Paths::for_test("/tmp/data", "/tmp/state");
    assert_eq!(paths.plugin_root.parent().unwrap(), paths.repo_cache_root.parent().unwrap());
    assert_eq!(
        paths.repo_cache_dir("github.com/user/repo"),
        paths.repo_cache_root.join("github.com/user/repo.git")
    );
}

#[test]
fn clear_failure_markers_removes_matching() {
    let dir = tempdir().unwrap();
    let failures_root = dir.path().join("failures");

    let marker1 = FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: build_command_hash("make"),
        build_command: "make".into(),
        failed_at: "2025-01-01T00:00:00Z".into(),
        stderr_summary: "err".into(),
    };
    let marker2 = FailureMarker {
        plugin_id: "github.com/other/plugin".into(),
        commit: "def456".into(),
        build_hash: build_command_hash("build"),
        build_command: "build".into(),
        failed_at: "2025-01-01T00:00:00Z".into(),
        stderr_summary: "err".into(),
    };

    write_failure_marker(&failures_root, &marker1).unwrap();
    write_failure_marker(&failures_root, &marker2).unwrap();

    clear_failure_markers(&failures_root, "github.com/user/repo").unwrap();

    let markers = read_failure_markers(&failures_root).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].plugin_id, "github.com/other/plugin");
}

#[test]
fn operation_lock_mutual_exclusion() {
    let dir = tempdir().unwrap();
    let lock_path = dir.path().join("operations.lock");

    let guard1 =
        OperationLock::try_acquire(&lock_path).unwrap().expect("should acquire first lock");

    // Second attempt should fail while first is held
    let guard2 = OperationLock::try_acquire(&lock_path).unwrap();
    assert!(guard2.is_none(), "should not acquire lock while held");

    drop(guard1);

    // Now it should be acquirable again
    let guard3 = OperationLock::try_acquire(&lock_path).unwrap();
    assert!(guard3.is_some(), "should acquire lock after release");
}
