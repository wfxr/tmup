use lazytmux::git::{publish_fresh_install, publish_replace};
use tempfile::tempdir;

#[test]
fn fresh_install_moves_staging_to_target() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("test.tmux"), "#!/bin/sh").unwrap();

    publish_fresh_install(&staging, &target, None).unwrap();

    assert!(!staging.exists(), "staging should be removed");
    assert!(target.exists(), "target should exist");
    assert!(target.join("test.tmux").exists());
}

#[test]
fn fresh_install_runs_build_on_success() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    std::fs::create_dir_all(&staging).unwrap();

    publish_fresh_install(&staging, &target, Some("touch built.marker")).unwrap();

    assert!(target.join("built.marker").exists());
}

#[test]
fn fresh_install_removes_target_on_build_failure() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("file.txt"), "content").unwrap();

    let result = publish_fresh_install(&staging, &target, Some("exit 1"));
    assert!(result.is_err());
    assert!(!target.exists(), "failed target should be cleaned up");
}

#[test]
fn replace_swaps_staging_into_target() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    // Create existing target
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("old.txt"), "old").unwrap();

    // Create staging
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("new.txt"), "new").unwrap();

    publish_replace(&staging, &target, None).unwrap();

    assert!(!staging.exists(), "staging should be removed");
    assert!(target.join("new.txt").exists(), "new content should be in target");
    assert!(!target.join("old.txt").exists(), "old content should be gone");
}

#[test]
fn replace_build_failure_leaves_existing_target_untouched() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    // Create existing target with old content
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("old.txt"), "old").unwrap();

    // Create staging with new content
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("new.txt"), "new").unwrap();

    let result = publish_replace(&staging, &target, Some("exit 1"));
    assert!(result.is_err());

    // Old content should remain because build fails before publish touches target.
    assert!(target.join("old.txt").exists(), "old content should remain installed");
    assert!(!target.join("new.txt").exists(), "new content should not remain");
}

#[test]
fn replace_with_successful_build() {
    let dir = tempdir().unwrap();
    let staging = dir.path().join("staging/plugin");
    let target = dir.path().join("plugins/github.com/user/repo");

    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("file.txt"), "content").unwrap();

    publish_replace(&staging, &target, Some("touch built.marker")).unwrap();

    assert!(target.join("built.marker").exists());
}
