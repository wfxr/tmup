use std::collections::{HashMap, HashSet};

use lazytmux::lockfile::{LockEntry, LockFile};
use lazytmux::model::Config;
use lazytmux::planner::{
    LastResult, PluginState, RepoHealth, collect_failed_builds, compute_statuses, plan_init,
};
use lazytmux::state::{FailureMarker, build_command_hash};
use tempfile::tempdir;

fn make_config(kdl: &str) -> Config {
    lazytmux::config::parse_config(kdl).unwrap()
}

#[test]
fn state_display_strings() {
    assert_eq!(PluginState::Installed.to_string(), "installed");
    assert_eq!(PluginState::Missing.to_string(), "missing");
    assert_eq!(PluginState::Outdated.to_string(), "outdated");
    assert_eq!(PluginState::PinnedTag.to_string(), "pinned-tag");
    assert_eq!(PluginState::PinnedCommit.to_string(), "pinned-commit");
}

#[test]
fn last_result_display_strings() {
    assert_eq!(LastResult::Ok.to_string(), "ok");
    assert_eq!(LastResult::BuildFailed.to_string(), "build-failed");
    assert_eq!(LastResult::None.to_string(), "none");
}

#[test]
fn read_only_init_plan_when_all_installed() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();

    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    assert!(plan.is_none());
}

#[test]
fn write_plan_when_plugin_missing() {
    let config = make_config(
        r#"
options { auto-install #true }
plugin "user/repo"
    "#,
    );
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> = HashMap::new();

    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected Some(WritePlan)");
    assert_eq!(plan.to_install, vec!["github.com/user/repo"]);
    assert!(plan.to_restore.is_empty());
    assert!(plan.to_clean.is_empty());
}

#[test]
fn auto_clean_detects_undeclared_plugins() {
    let config = make_config(
        r#"
options { auto-clean #true }
plugin "user/repo"
    "#,
    );
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();
    let managed_ids: HashSet<String> =
        ["github.com/user/repo", "github.com/old/removed"].iter().map(|s| s.to_string()).collect();

    let plan = plan_init(&config, &lock, &health, &managed_ids);
    let plan = plan.expect("expected Some(WritePlan)");
    assert!(plan.to_install.is_empty());
    assert!(plan.to_restore.is_empty());
    assert_eq!(plan.to_clean, vec!["github.com/old/removed"]);
}

#[test]
fn restore_plan_when_installed_commit_drifted() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    // Installed at a different commit than the lock
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "def456".into() })].into();

    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected Some(WritePlan) with to_restore");
    assert!(plan.to_install.is_empty());
    assert_eq!(plan.to_restore, vec!["github.com/user/repo"]);
    assert!(plan.to_clean.is_empty());
}

#[test]
fn build_failure_keeps_state_and_result_separate() {
    let config = make_config(r#"plugin "user/repo" build="make""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();

    let bh = build_command_hash("make");
    let marker = FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: bh,
        build_command: "make".into(),
        failed_at: "2025-01-01T00:00:00Z".into(),
        stderr_summary: "error".into(),
    };
    let failed_builds = collect_failed_builds(&[marker]);

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Installed);
    assert_eq!(statuses[0].last_result, LastResult::BuildFailed);
}

#[test]
fn missing_plugin_with_build_failure_shows_missing_and_failed() {
    let config = make_config(r#"plugin "user/repo" build="make""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> = HashMap::new();

    let bh = build_command_hash("make");
    let marker = FailureMarker {
        plugin_id: "github.com/user/repo".into(),
        commit: "abc123".into(),
        build_hash: bh,
        build_command: "make".into(),
        failed_at: "2025-01-01T00:00:00Z".into(),
        stderr_summary: "error".into(),
    };
    let failed_builds = collect_failed_builds(&[marker]);

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Missing);
    assert_eq!(statuses[0].last_result, LastResult::BuildFailed);
}

#[test]
fn local_plugin_status() {
    let dir = tempdir().unwrap();
    let plugin_dir = dir.path().join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let config = make_config(&format!(r#"plugin "{}" local=#true"#, plugin_dir.display()));
    let lock = LockFile::new();
    let health = HashMap::new();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Local);
    assert_eq!(statuses[0].last_result, LastResult::Ok);
    assert_eq!(statuses[0].kind, "local");
}

#[test]
fn missing_local_plugin_shows_missing_and_none() {
    let dir = tempdir().unwrap();
    let plugin_dir = dir.path().join("missing-plugin");
    let config = make_config(&format!(r#"plugin "{}" local=#true"#, plugin_dir.display()));
    let lock = LockFile::new();
    let health = HashMap::new();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Missing);
    assert_eq!(statuses[0].last_result, LastResult::None);
    assert_eq!(statuses[0].kind, "local");
}

#[test]
fn local_plugin_file_path_shows_broken_and_none() {
    let dir = tempdir().unwrap();
    let plugin_file = dir.path().join("plugin.tmux");
    std::fs::write(&plugin_file, "#!/bin/sh\n").unwrap();
    let config = make_config(&format!(r#"plugin "{}" local=#true"#, plugin_file.display()));
    let lock = LockFile::new();
    let health = HashMap::new();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Broken);
    assert_eq!(statuses[0].last_result, LastResult::None);
    assert_eq!(statuses[0].kind, "local");
}

#[test]
fn pinned_tag_status() {
    let config = make_config(r#"plugin "user/repo" tag="v1.0""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::tag("v1.0", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::PinnedTag);
}

#[test]
fn pinned_tag_with_drifted_head_shows_outdated() {
    let config = make_config(r#"plugin "user/repo" tag="v1.0""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::tag("v1.0", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "def456".into() })].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Outdated);
    assert_eq!(statuses[0].current_commit.as_deref(), Some("def456"));
    assert_eq!(statuses[0].lock_commit.as_deref(), Some("abc123"));
}

#[test]
fn pinned_commit_with_drifted_head_shows_outdated() {
    let config = make_config(r#"plugin "user/repo" commit="abc123""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::commit("abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "def456".into() })].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Outdated);
    assert_eq!(statuses[0].current_commit.as_deref(), Some("def456"));
    assert_eq!(statuses[0].lock_commit.as_deref(), Some("abc123"));
}

#[test]
fn outdated_state_when_installed_commit_differs_from_lock() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "def456".into() })].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Outdated);
    assert_eq!(statuses[0].current_commit.as_deref(), Some("def456"));
    assert_eq!(statuses[0].lock_commit.as_deref(), Some("abc123"));
}

#[test]
fn inspect_missing_dir() {
    let dir = tempdir().unwrap();
    let health = lazytmux::planner::inspect_plugin_dir(&dir.path().join("nonexistent"));
    assert!(matches!(health, RepoHealth::Missing));
}

#[test]
fn inspect_healthy_git_repo() {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("plugin");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    std::fs::write(repo.join("init.tmux"), "#!/bin/sh\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .unwrap();
    let health = lazytmux::planner::inspect_plugin_dir(&repo);
    assert!(matches!(health, RepoHealth::Healthy { .. }));
    if let RepoHealth::Healthy { commit } = health {
        assert_eq!(commit.len(), 40);
    }
}

#[test]
fn inspect_dir_exists_no_git() {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("plugin");
    std::fs::create_dir_all(&repo).unwrap();
    let health = lazytmux::planner::inspect_plugin_dir(&repo);
    assert!(matches!(health, RepoHealth::Broken));
}

#[test]
fn inspect_dir_with_empty_dotgit() {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("plugin");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let health = lazytmux::planner::inspect_plugin_dir(&repo);
    assert!(matches!(health, RepoHealth::Broken));
}

#[test]
fn scan_managed_finds_git_dirs() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("plugins");
    std::fs::create_dir_all(root.join("github.com/user/repo-a/.git")).unwrap();
    std::fs::create_dir_all(root.join("github.com/user/repo-b/.git")).unwrap();
    std::fs::create_dir_all(root.join("github.com/user/not-a-repo")).unwrap();
    let ids = lazytmux::planner::scan_managed_plugin_ids(&root);
    assert!(ids.contains("github.com/user/repo-a"));
    assert!(ids.contains("github.com/user/repo-b"));
    assert!(!ids.contains("github.com/user/not-a-repo"));
    assert_eq!(ids.len(), 2);
}

#[test]
fn broken_state_display_string() {
    assert_eq!(lazytmux::planner::PluginState::Broken.to_string(), "broken");
}

#[test]
fn broken_repo_shows_broken_in_list() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Broken)].into();
    let failed_builds = HashSet::new();
    let statuses = compute_statuses(&config, &lock, &health, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Broken);
    assert_eq!(statuses[0].last_result, LastResult::None);
}

#[test]
fn init_plans_restore_for_broken_plugin_with_lock() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert("github.com/user/repo".into(), LockEntry::branch("main", "abc123"));
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Broken)].into();
    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected WritePlan");
    assert!(plan.to_install.is_empty());
    assert_eq!(plan.to_restore, vec!["github.com/user/repo"]);
}

#[test]
fn init_plans_install_for_broken_plugin_without_lock() {
    let config = make_config(
        r#"
options { auto-install #true }
plugin "user/repo"
    "#,
    );
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Broken)].into();
    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected WritePlan");
    assert_eq!(plan.to_install, vec!["github.com/user/repo"]);
    assert!(plan.to_restore.is_empty());
}

#[test]
fn init_plans_install_for_healthy_plugin_without_lock() {
    let config = make_config(r#"plugin "user/repo""#);
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();
    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected WritePlan — Healthy+no-lock needs install");
    assert_eq!(plan.to_install, vec!["github.com/user/repo"]);
    assert!(plan.to_restore.is_empty());
}

#[test]
fn init_does_not_install_healthy_unlocked_plugin_when_auto_install_disabled() {
    let config = make_config(
        r#"
options { auto-install #false }
plugin "user/repo"
    "#,
    );
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> =
        [("github.com/user/repo".into(), RepoHealth::Healthy { commit: "abc123".into() })].into();

    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    assert!(plan.is_none());
}

#[test]
fn init_plan_follows_config_declaration_order() {
    let config = make_config(
        r#"
options { auto-install #true }
plugin "user/alpha"
plugin "user/beta"
plugin "user/gamma"
    "#,
    );
    let lock = LockFile::new();
    let health: HashMap<String, RepoHealth> = HashMap::new();
    let plan = plan_init(&config, &lock, &health, &HashSet::new());
    let plan = plan.expect("expected WritePlan");
    assert_eq!(
        plan.to_install,
        vec!["github.com/user/alpha", "github.com/user/beta", "github.com/user/gamma",]
    );
}
