use std::collections::{HashMap, HashSet};

use lazytmux::{
    lockfile::{LockEntry, LockFile},
    model::Config,
    planner::{
        InitDecision,
        LastResult,
        PluginState,
        collect_failed_builds,
        compute_statuses,
        plan_init,
    },
    state::{FailureMarker, build_command_hash},
};

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
    assert_eq!(PluginState::Unmanaged.to_string(), "unmanaged");
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
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("abc123".into()))].into();

    let decision = plan_init(&config, &lock, &installed, false);
    assert_eq!(decision, InitDecision::ReadOnly);
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
    let installed: HashMap<String, Option<String>> = HashMap::new();

    let decision = plan_init(&config, &lock, &installed, false);
    match decision {
        InitDecision::Write(plan) => {
            assert_eq!(plan.to_install, vec!["github.com/user/repo"]);
            assert!(plan.to_restore.is_empty());
            assert!(plan.to_clean.is_empty());
        }
        other => panic!("expected Write, got {other:?}"),
    }
}

#[test]
fn wait_for_writer_when_read_only_and_writer_active() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("abc123".into()))].into();

    let decision = plan_init(&config, &lock, &installed, true);
    assert_eq!(decision, InitDecision::WaitForWriter);
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
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> = [
        ("github.com/user/repo".into(), Some("abc123".into())),
        ("github.com/old/removed".into(), Some("def456".into())),
    ]
    .into();

    let decision = plan_init(&config, &lock, &installed, false);
    match decision {
        InitDecision::Write(plan) => {
            assert!(plan.to_install.is_empty());
            assert!(plan.to_restore.is_empty());
            assert_eq!(plan.to_clean, vec!["github.com/old/removed"]);
        }
        other => panic!("expected Write, got {other:?}"),
    }
}

#[test]
fn restore_plan_when_installed_commit_drifted() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    // Installed at a different commit than the lock
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("def456".into()))].into();

    let decision = plan_init(&config, &lock, &installed, false);
    match decision {
        InitDecision::Write(plan) => {
            assert!(plan.to_install.is_empty());
            assert_eq!(plan.to_restore, vec!["github.com/user/repo"]);
            assert!(plan.to_clean.is_empty());
        }
        other => panic!("expected Write with to_restore, got {other:?}"),
    }
}

#[test]
fn build_failure_keeps_state_and_result_separate() {
    let config = make_config(r#"plugin "user/repo" build="make""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("abc123".into()))].into();

    let bh = build_command_hash("make");
    let marker = FailureMarker {
        plugin_id:      "github.com/user/repo".into(),
        commit:         "abc123".into(),
        build_hash:     bh,
        build_command:  "make".into(),
        failed_at:      "2025-01-01T00:00:00Z".into(),
        stderr_summary: "error".into(),
    };
    let failed_builds = collect_failed_builds(&[marker]);

    let statuses = compute_statuses(&config, &lock, &installed, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Installed);
    assert_eq!(statuses[0].last_result, LastResult::BuildFailed);
}

#[test]
fn missing_plugin_with_build_failure_shows_missing_and_failed() {
    let config = make_config(r#"plugin "user/repo" build="make""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> = HashMap::new();

    let bh = build_command_hash("make");
    let marker = FailureMarker {
        plugin_id:      "github.com/user/repo".into(),
        commit:         "abc123".into(),
        build_hash:     bh,
        build_command:  "make".into(),
        failed_at:      "2025-01-01T00:00:00Z".into(),
        stderr_summary: "error".into(),
    };
    let failed_builds = collect_failed_builds(&[marker]);

    let statuses = compute_statuses(&config, &lock, &installed, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Missing);
    assert_eq!(statuses[0].last_result, LastResult::BuildFailed);
}

#[test]
fn local_plugin_status() {
    let config = make_config(r#"plugin "~/dev/my-plugin" local=#true"#);
    let lock = LockFile::new();
    let installed = HashMap::new();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &installed, &failed_builds);
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, PluginState::Local);
    assert_eq!(statuses[0].kind, "local");
}

#[test]
fn pinned_tag_status() {
    let config = make_config(r#"plugin "user/repo" tag="v1.0""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::tag("user/repo", "v1.0", "abc123"),
    );
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("abc123".into()))].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &installed, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::PinnedTag);
}

#[test]
fn outdated_state_when_installed_commit_differs_from_lock() {
    let config = make_config(r#"plugin "user/repo""#);
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/user/repo".into(),
        LockEntry::branch("user/repo", "main", "abc123"),
    );
    let installed: HashMap<String, Option<String>> =
        [("github.com/user/repo".into(), Some("def456".into()))].into();
    let failed_builds = HashSet::new();

    let statuses = compute_statuses(&config, &lock, &installed, &failed_builds);
    assert_eq!(statuses[0].state, PluginState::Outdated);
    assert_eq!(statuses[0].current_commit.as_deref(), Some("def456"));
    assert_eq!(statuses[0].lock_commit.as_deref(), Some("abc123"));
}
