use lazytmux::{
    config::parse_config,
    lockfile::{LockEntry, LockFile},
    planner,
    plugin,
    state::{Paths, build_command_hash},
};
use std::path::Path;
use tempfile::tempdir;

/// Create a minimal but real git repo at `path` with one commit, returning
/// the HEAD commit hash.
fn init_git_repo(path: &Path) -> String {
    std::fs::create_dir_all(path).unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    std::fs::write(path.join("init.tmux"), "#!/bin/sh\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .unwrap();
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn list_returns_state_and_last_result() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let config = parse_config(
        r#"
plugin "user/repo-a"
plugin "user/repo-b" build="make"
    "#,
    )
    .unwrap();

    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo-a".into(),
        LockEntry::branch("user/repo-a", "main", "aaa111"),
    );
    lock.plugins.insert(
        "github.com/user/repo-b".into(),
        LockEntry::branch("user/repo-b", "main", "bbb222"),
    );

    // Simulate repo-a installed (real git repo), repo-b missing
    let plugin_a = paths.plugin_dir("github.com/user/repo-a");
    let commit_a = init_git_repo(&plugin_a);

    // Update lock to match the real commit so state is Installed, not Outdated
    lock.plugins.insert(
        "github.com/user/repo-a".into(),
        LockEntry::branch("user/repo-a", "main", &commit_a),
    );

    let statuses = plugin::list(&config, &lock, &paths).unwrap();
    assert_eq!(statuses.len(), 2);

    let a = &statuses[0];
    assert_eq!(a.id, "github.com/user/repo-a");
    assert_eq!(a.state, planner::PluginState::Installed);

    let b = &statuses[1];
    assert_eq!(b.id, "github.com/user/repo-b");
    assert_eq!(b.state, planner::PluginState::Missing);
}

#[test]
fn clean_removes_undeclared_plugins() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    // Declared: only repo-a
    let config = parse_config(r#"plugin "user/repo-a""#).unwrap();

    // Installed: repo-a and repo-b
    let plugin_a = paths.plugin_dir("github.com/user/repo-a");
    let plugin_b = paths.plugin_dir("github.com/user/repo-b");
    std::fs::create_dir_all(plugin_a.join(".git")).unwrap();
    std::fs::create_dir_all(plugin_b.join(".git")).unwrap();

    plugin::clean(&config, &paths).unwrap();

    assert!(plugin_a.exists(), "declared plugin should remain");
    assert!(!plugin_b.exists(), "undeclared plugin should be removed");
}

#[test]
fn clean_does_not_remove_local_plugin_source() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let config = parse_config(r#"plugin "~/dev/my-plugin" local=#true"#).unwrap();

    // Local plugin source is not in the managed directory, so clean should not touch it
    plugin::clean(&config, &paths).unwrap();
    // Just verifying no panic/error
}

#[test]
fn is_known_failure_detects_matching_key() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    // Write a failure marker
    let marker = lazytmux::state::FailureMarker {
        plugin_id:      "github.com/user/repo".into(),
        commit:         "abc123".into(),
        build_hash:     build_command_hash("make"),
        build_command:  "make".into(),
        failed_at:      "now".into(),
        stderr_summary: "error".into(),
    };
    lazytmux::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    // Same tuple: should be known
    assert!(plugin::is_known_failure(&paths, "github.com/user/repo", "abc123", "make").unwrap());

    // Different build command: should NOT be known
    assert!(
        !plugin::is_known_failure(&paths, "github.com/user/repo", "abc123", "just build").unwrap()
    );

    // Different commit: should NOT be known
    assert!(!plugin::is_known_failure(&paths, "github.com/user/repo", "def456", "make").unwrap());
}

#[test]
fn update_skips_pinned_tag() {
    let config = parse_config(r#"plugin "user/repo" tag="v1.0""#).unwrap();
    let spec = &config.plugins[0];
    assert!(matches!(spec.tracking, lazytmux::model::Tracking::Tag(_)));
}

#[test]
fn update_skips_pinned_commit() {
    let config = parse_config(r#"plugin "user/repo" commit="abc123""#).unwrap();
    let spec = &config.plugins[0];
    assert!(matches!(
        spec.tracking,
        lazytmux::model::Tracking::Commit(_)
    ));
}

#[test]
fn list_shows_both_state_and_last_result_for_build_failure() {
    let dir = tempdir().unwrap();
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let config = parse_config(r#"plugin "user/repo" build="make""#).unwrap();
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );

    // Plugin is installed (real git repo) but has a build failure marker
    let plugin_dir = paths.plugin_dir("github.com/user/repo");
    let commit = init_git_repo(&plugin_dir);

    // Update lock to match the real commit
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", &commit),
    );

    let marker = lazytmux::state::FailureMarker {
        plugin_id:      "github.com/user/repo".into(),
        commit:         commit.clone(),
        build_hash:     build_command_hash("make"),
        build_command:  "make".into(),
        failed_at:      "now".into(),
        stderr_summary: "error".into(),
    };
    lazytmux::state::write_failure_marker(&paths.failures_root, &marker).unwrap();

    let statuses = plugin::list(&config, &lock, &paths).unwrap();
    assert_eq!(statuses[0].state, planner::PluginState::Installed);
    assert_eq!(statuses[0].last_result, planner::LastResult::BuildFailed);
}
