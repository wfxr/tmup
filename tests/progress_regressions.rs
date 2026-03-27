mod utils;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::tempdir;
use utils::git;

fn make_remote_repo(root: &Path) -> PathBuf {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();

    git(&["init", "-b", "main"], &work);
    std::fs::write(work.join("init.tmux"), "#!/bin/sh\n").unwrap();
    git(&["add", "."], &work);
    git(&["commit", "-m", "init"], &work);

    let bare_parent = root.join("remotes/example.com/test");
    std::fs::create_dir_all(&bare_parent).unwrap();
    let bare = bare_parent.join("plugin.git");
    git(&["clone", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()], root);
    bare
}

fn write_git_rewrite_config(root: &Path) -> PathBuf {
    let gitconfig = root.join("gitconfig");
    let rewritten_base = format!("file://{}/", root.join("remotes/example.com").display());
    std::fs::write(
        &gitconfig,
        format!("[url \"{rewritten_base}\"]\n    insteadOf = https://example.com/\n"),
    )
    .unwrap();
    gitconfig
}

fn write_fake_tmux_with_log(root: &Path, log_path: &Path) -> PathBuf {
    let bin_dir = root.join("bin-with-log");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("tmux");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{log}"
case "$1" in
  -V) printf 'tmux 3.3a\n'; exit 0 ;;
  set-environment|set|run-shell|wait-for|display-message|display-popup|split-window|set-option) exit 0 ;;
  *) exit 0 ;;
esac
"#,
            log = log_path.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    bin_dir
}

fn write_fake_tmux_bootstrap_with_log(
    root: &Path,
    log_path: &Path,
    popup_status: i32,
    split_status: i32,
) -> PathBuf {
    let bin_dir = root.join(format!("bin-bootstrap-{popup_status}-{split_status}"));
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("tmux");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{log}"
case "$1" in
  -V) printf 'tmux 3.3a\n'; exit 0 ;;
  display-message)
    if [ "$2" = "-p" ]; then
      case "$3" in
        '#{{client_name}}') printf '/dev/pts/99\n'; exit 0 ;;
        '#{{pane_id}}') printf '%%42\n'; exit 0 ;;
      esac
    fi
    exit 0 ;;
  display-popup) exit {popup_status} ;;
  split-window) printf '%%42\n'; exit {split_status} ;;
  run-shell|wait-for|set-environment|set|set-option) exit 0 ;;
  *) exit 0 ;;
esac
"#,
            log = log_path.display(),
            popup_status = popup_status,
            split_status = split_status,
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    bin_dir
}

fn write_fake_tmux_versioned_with_log(
    root: &Path,
    log_path: &Path,
    version: &str,
    popup_status: i32,
    split_status: i32,
) -> PathBuf {
    let safe_version = version.replace([' ', '.'], "-");
    let bin_dir = root.join(format!("bin-versioned-{safe_version}-{popup_status}-{split_status}"));
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("tmux");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{log}"
case "$1" in
  -V) printf '%s\n' '{version}'; exit 0 ;;
  display-message)
    if [ "$2" = "-p" ]; then
      case "$3" in
        '#{{client_name}}') printf '/dev/pts/99\n'; exit 0 ;;
        '#{{pane_id}}') printf '%%42\n'; exit 0 ;;
      esac
    fi
    exit 0 ;;
  display-popup) exit {popup_status} ;;
  split-window) exit {split_status} ;;
  run-shell|wait-for|set-environment|set|set-option) exit 0 ;;
  *) exit 0 ;;
esac
"#,
            log = log_path.display(),
            version = version,
            popup_status = popup_status,
            split_status = split_status,
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    bin_dir
}

fn write_fake_tmux_retry_probe_with_log(root: &Path, log_path: &Path) -> PathBuf {
    write_fake_tmux_retry_probe_until_with_log(root, log_path, 1)
}

fn write_fake_tmux_retry_probe_after_delay_with_log(
    root: &Path,
    log_path: &Path,
    ready_after_ms: u64,
) -> PathBuf {
    let bin_dir = root.join(format!("bin-retry-probe-after-{ready_after_ms}ms"));
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("tmux");
    let start_ms_file = root.join("probe-start-ms");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
start_ms_file="{start_ms_file}"
printf '%s\n' "$*" >> "{log}"
now_ms=$(date +%s%3N)
case "$1" in
  -V) printf 'tmux 3.3a\n'; exit 0 ;;
  display-message)
    if [ "$2" = "-p" ]; then
      if [ ! -f "$start_ms_file" ]; then
        printf '%s' "$now_ms" > "$start_ms_file"
      fi
      start_ms=$(cat "$start_ms_file")
      elapsed_ms=$((now_ms - start_ms))
      if [ "$elapsed_ms" -lt {ready_after_ms} ]; then
        printf '%s\n' 'no current client' >&2
        exit 1
      fi
      case "$3" in
        '#{{client_name}}') printf '/dev/pts/99\n'; exit 0 ;;
        '#{{pane_id}}') printf '%%42\n'; exit 0 ;;
      esac
    fi
    exit 0 ;;
  display-popup) exit 0 ;;
  split-window) printf '%%42\n'; exit 1 ;;
  run-shell|wait-for|set-environment|set|set-option) exit 0 ;;
  *) exit 0 ;;
esac
"#,
            start_ms_file = start_ms_file.display(),
            log = log_path.display(),
            ready_after_ms = ready_after_ms,
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    bin_dir
}

fn write_fake_tmux_retry_probe_until_with_log(
    root: &Path,
    log_path: &Path,
    fail_display_message_calls: usize,
) -> PathBuf {
    let bin_dir = root.join("bin-retry-probe");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = bin_dir.join("tmux");
    let probe_count = root.join("probe-count");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
count_file="{count_file}"
printf '%s\n' "$*" >> "{log}"
case "$1" in
  -V) printf 'tmux 3.3a\n'; exit 0 ;;
  display-message)
    if [ "$2" = "-p" ]; then
      count=0
      [ -f "$count_file" ] && count=$(cat "$count_file")
      count=$((count + 1))
      printf '%s' "$count" > "$count_file"
      if [ "$count" -le {fail_display_message_calls} ]; then
        printf '%s\n' 'no current client' >&2
        exit 1
      fi
      case "$3" in
        '#{{client_name}}') printf '/dev/pts/99\n'; exit 0 ;;
        '#{{pane_id}}') printf '%%42\n'; exit 0 ;;
      esac
    fi
    exit 0 ;;
  display-popup) exit 0 ;;
  split-window) printf '%%42\n'; exit 1 ;;
  run-shell|wait-for|set-environment|set|set-option) exit 0 ;;
  *) exit 0 ;;
esac
"#,
            count_file = probe_count.display(),
            log = log_path.display(),
            fail_display_message_calls = fail_display_message_calls,
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    bin_dir
}

#[cfg(unix)]
#[test]
fn sync_surfaces_lockfile_write_failure() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    let config_dir = dir.path().join("alt-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("custom.kdl");
    std::fs::write(&config_path, r#"plugin "https://example.com/test/plugin.git""#).unwrap();

    let original_mode = std::fs::metadata(&config_dir).unwrap().permissions().mode();
    let mut readonly = std::fs::metadata(&config_dir).unwrap().permissions();
    readonly.set_mode(0o555);
    std::fs::set_permissions(&config_dir, readonly).unwrap();

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("sync")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("HOME", dir.path())
        .output()
        .unwrap();

    let mut restored = std::fs::metadata(&config_dir).unwrap().permissions();
    restored.set_mode(original_mode);
    std::fs::set_permissions(&config_dir, restored).unwrap();

    assert!(!output.status.success(), "sync should fail when lockfile cannot be persisted");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to create")
            || stderr.contains("failed to rename")
            || stderr.contains("Permission denied"),
        "stderr should include the real lockfile write error, got:\n{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn init_ui_child_stops_after_sync_failure() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "https://example.com/test/plugin.git""#).unwrap();

    let original_mode = std::fs::metadata(&config_dir).unwrap().permissions().mode();
    let mut readonly = std::fs::metadata(&config_dir).unwrap().permissions();
    readonly.set_mode(0o555);
    std::fs::set_permissions(&config_dir, readonly).unwrap();

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--ui-child",
            "--wait-channel",
            "test-channel",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("HOME", dir.path())
        .output()
        .unwrap();

    let mut restored = std::fs::metadata(&config_dir).unwrap().permissions();
    restored.set_mode(original_mode);
    std::fs::set_permissions(&config_dir, restored).unwrap();

    assert!(!output.status.success(), "ui child should fail when sync fails");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Fetching"), "stderr should show sync started, got:\n{stderr}");
    assert!(
        !stderr.contains("Loading tmux applying load plan"),
        "init should not show a tmux-loading stage after sync failure, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Failed operation")
            || stderr.contains("failed to create")
            || stderr.contains("failed to rename")
            || stderr.contains("Permission denied"),
        "stderr should show an operation-level failure, got:\n{stderr}"
    );
}

#[test]
fn init_ui_child_stops_when_remote_is_missing_during_fetch() {
    let dir = tempdir().unwrap();
    let bare = make_remote_repo(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());
    // Remove the bare repo to simulate remote disappearing after cache population.
    std::fs::remove_dir_all(&bare).unwrap();

    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "https://example.com/test/plugin.git""#).unwrap();

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--ui-child",
            "--wait-channel",
            "test-channel",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("HOME", dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "init should fail when fetch cannot reach remote");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Fetching"),
        "stderr should show sync started even when remote is missing, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Failed operation")
            || stderr.contains("git clone --bare failed")
            || stderr.contains("failed to run git fetch origin")
            || stderr.contains("No such file or directory"),
        "stderr should expose the clone/fetch failure, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("Loading tmux applying load plan"),
        "init should skip tmux loading when sync cannot fetch the remote, got:\n{stderr}"
    );
}

#[test]
fn init_parent_schedules_bootstrap_in_background() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_with_log(dir.path(), &tmux_log);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(output.status.success(), "init should succeed after scheduling bootstrap");

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    assert!(
        log.contains("run-shell -b "),
        "expected init parent to schedule bootstrap with run-shell -b, got log:\n{log}"
    );
    assert!(
        log.contains("'init' '--bootstrap'"),
        "expected scheduled bootstrap command in tmux log, got log:\n{log}"
    );
    assert!(
        !log.contains("display-popup "),
        "init parent should not open popup synchronously anymore, got log:\n{log}"
    );
    assert!(
        !log.contains("split-window "),
        "init parent should not open split synchronously anymore, got log:\n{log}"
    );
}

#[test]
fn init_parent_uses_immediate_ui_when_attached_client_is_ready() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_bootstrap_with_log(dir.path(), &tmux_log, 0, 1);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "attached init should synchronously observe popup child failure"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    assert!(
        log.contains("display-popup "),
        "attached init should use popup immediately when a client target is already available, got log:\n{log}"
    );
    assert!(
        !log.contains("run-shell -b "),
        "attached init should not defer to background bootstrap, got log:\n{log}"
    );
}

#[test]
fn init_parent_missing_popup_result_includes_popup_context() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_bootstrap_with_log(dir.path(), &tmux_log, 0, 1);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "attached init should fail when popup succeeds without producing a result file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reading popup init result"),
        "stderr should include popup result context, got:\n{stderr}"
    );
}

#[test]
fn init_parent_tmux_3_2_popup_omits_title_flag() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_versioned_with_log(dir.path(), &tmux_log, "tmux 3.2", 0, 1);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "attached init should still fail when popup leaves no result file"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    let popup_header = log
        .lines()
        .find(|line| line.starts_with("display-popup "))
        .unwrap_or_else(|| panic!("expected display-popup call, got:\n{log}"));
    assert!(
        popup_header.contains("display-popup -E -w 80% -h 80% -c /dev/pts/99 -- "),
        "tmux 3.2 popup should still be used, got:\n{popup_header}"
    );
    assert!(
        !popup_header.contains("-T lazytmux init"),
        "tmux 3.2 popup should omit the title flag, got:\n{popup_header}"
    );
}

#[test]
fn init_parent_uses_split_only_when_tmux_is_2_0() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_versioned_with_log(dir.path(), &tmux_log, "tmux 2.0", 1, 0);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "split-only init should still fail when split child leaves no result file"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    assert!(!log.contains("display-popup "), "tmux 2.0 should not attempt popup, got log:\n{log}");
    assert!(log.contains("split-window "), "tmux 2.0 should use split-window, got log:\n{log}");
}

#[test]
fn init_parent_uses_inline_when_tmux_is_1_9() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_versioned_with_log(dir.path(), &tmux_log, "tmux 1.9", 1, 1);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let _output = Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("init")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    assert!(
        !log.contains("display-popup "),
        "tmux 1.9 inline mode should not attempt popup, got log:\n{log}"
    );
    assert!(
        !log.contains("split-window "),
        "tmux 1.9 inline mode should not attempt split-window, got log:\n{log}"
    );
}

#[test]
fn init_bootstrap_uses_split_when_tmux_is_2_0() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_versioned_with_log(dir.path(), &tmux_log, "tmux 2.0", 1, 0);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--bootstrap",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "bootstrap should fail when split child leaves no result file"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    let split_index = log.find("split-window -v -l 50% -t %42 -- ");
    let inline_option_index =
        log.find("tmux set-option -p remain-on-exit failed >/dev/null 2>&1 || true");
    assert!(
        split_index.is_some(),
        "expected tmux 2.0 bootstrap to use split-window, got log:\n{log}"
    );
    assert!(
        !log.contains("display-popup "),
        "tmux 2.0 bootstrap should not attempt popup, got log:\n{log}"
    );
    assert!(
        inline_option_index.is_some(),
        "split wrapper should set remain-on-exit before launching the child, got log:\n{log}"
    );
    let split_index = split_index.unwrap();
    let inline_option_index = inline_option_index.unwrap();
    let child_index = log[split_index..]
        .find(" init --ui-child --wait-channel ")
        .map(|offset| split_index + offset)
        .expect("expected child command in split-window tmux log");
    assert!(
        split_index < inline_option_index && inline_option_index < child_index,
        "split wrapper should set remain-on-exit before launching the child, got log:\n{log}"
    );
    assert!(
        log.lines().any(|line| line.starts_with("wait-for ")),
        "split path should still wait for the child signal, got log:\n{log}"
    );
    let key_wait_index = log.find("dd bs=1 count=1").unwrap_or_else(|| {
        panic!("split wrapper should wait for keypress before closing, got log:\n{log}")
    });
    let wait_index = log
        .rfind("\nwait-for ")
        .map(|offset| offset + 1)
        .unwrap_or_else(|| panic!("split path should signal completion, got log:\n{log}"));
    assert!(
        key_wait_index < wait_index,
        "split wrapper should wait for keypress before the parent wait-for returns, got log:\n{log}"
    );
}

/// Regression test for the popup path: verifies that
/// 1. the wrapper is passed as a single shell-command (no extra `sh -c`),
/// 2. `wait-for` is NOT called after `display-popup` (it blocks until close).
#[test]
fn init_bootstrap_popup_path_targets_explicit_client_and_skips_wait_for() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_bootstrap_with_log(dir.path(), &tmux_log, 0, 1);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--bootstrap",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    // Bootstrap fails because the fake tmux doesn't actually run the child (no result file).
    assert!(!output.status.success());

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();

    // display-popup was called (the wrapper is multi-line, so the entry spans
    // several lines in the log — match the first line that starts with
    // "display-popup" and use the full log for content checks).
    let popup_start = log
        .find("display-popup ")
        .unwrap_or_else(|| panic!("expected display-popup call, got:\n{log}"));
    let popup_header =
        &log[popup_start..log[popup_start..].find('\n').map_or(log.len(), |i| popup_start + i)];

    // Wrapper is passed directly — no extra `sh -c` between `--` and the wrapper body.
    assert!(
        popup_header.contains(
            "display-popup -E -w 80% -h 80% -c /dev/pts/99 -T  lazytmux init (press #[bold,fg=red]q#[default] to exit)  -- "
        ),
        "display-popup should target the probed client explicitly with the new argument shape, got:\n{popup_header}"
    );
    assert!(
        !popup_header.contains(" -- sh -c "),
        "display-popup should NOT wrap command in sh -c (tmux does this internally), got:\n{popup_header}"
    );

    // The wrapper (multi-line) must contain the child command.
    assert!(
        log[popup_start..].contains(" init --ui-child --wait-channel "),
        "display-popup wrapper should contain the child command, got log:\n{log}"
    );
    let key_wait_index = log[popup_start..]
        .find("dd bs=1 count=1")
        .map(|offset| popup_start + offset)
        .unwrap_or_else(|| {
            panic!("popup wrapper should wait for keypress before exit, got log:\n{log}")
        });
    let result_index = log[popup_start..]
        .find("printf '{\"exit_code\":%d}\\n' \"$rc\" > \"$result_file\"")
        .map(|offset| popup_start + offset)
        .expect("popup wrapper should write the result file");
    assert!(
        result_index < key_wait_index,
        "popup wrapper should write the result before waiting for keypress, got log:\n{log}"
    );
    assert!(
        log[popup_start..].contains("exit 0"),
        "popup wrapper should exit cleanly after q closes the UI, got log:\n{log}"
    );

    // wait-for must NOT be called on the popup path (display-popup blocks until close).
    assert!(
        !log.lines().any(|line| line.starts_with("wait-for ")),
        "popup path should not call wait-for (display-popup already blocks), got log:\n{log}"
    );
}

#[test]
fn init_bootstrap_retries_probe_until_target_is_ready() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_retry_probe_with_log(dir.path(), &tmux_log);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--bootstrap",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "bootstrap should still fail once popup child leaves no result file"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    let probe_calls = log.lines().filter(|line| line.starts_with("display-message -p ")).count();
    assert!(
        probe_calls >= 3,
        "bootstrap should retry target probing after a transient display-message failure, got log:\n{log}"
    );
    assert!(
        log.contains("display-popup "),
        "bootstrap should eventually reach popup after retrying the probe, got log:\n{log}"
    );
}

#[test]
fn init_bootstrap_keeps_probing_long_enough_for_late_target() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir =
        write_fake_tmux_retry_probe_after_delay_with_log(dir.path(), &tmux_log, 1000);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--bootstrap",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            dir.path().join("data").to_str().unwrap(),
            "--state-root",
            dir.path().join("state").to_str().unwrap(),
        ])
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "bootstrap should still fail once popup child leaves no result file"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    assert!(
        log.contains("display-popup "),
        "bootstrap should still reach popup when the target appears near the end of the retry window, got log:\n{log}"
    );
}

#[test]
fn init_loads_tmux_after_sync_plugin_failures() {
    let dir = tempdir().unwrap();
    let _bare = make_remote_repo(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(
        &config_path,
        r#"
options { auto-install #true }
plugin "https://example.com/test/plugin.git" build="exit 1"
        "#,
    )
    .unwrap();

    let data_root = dir.path().join("data");
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();

    let tmux_log = dir.path().join("tmux.log");
    let fake_tmux_dir = write_fake_tmux_with_log(dir.path(), &tmux_log);
    let path = format!("{}:{}", fake_tmux_dir.display(), std::env::var("PATH").unwrap_or_default());

    let output = Command::cargo_bin("lazytmux")
        .unwrap()
        .args([
            "init",
            "--ui-child",
            "--wait-channel",
            "test-channel",
            "--config-path",
            config_path.to_str().unwrap(),
            "--data-root",
            data_root.to_str().unwrap(),
            "--state-root",
            state_root.to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("HOME", dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "ui child should still return non-zero after plugin-level sync failures"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Fetching"), "stderr:\n{stderr}");
    assert!(
        !stderr.contains("Loading tmux applying load plan"),
        "tmux loading stage should stay silent even when init continues after plugin failures, got:\n{stderr}"
    );

    let log = std::fs::read_to_string(&tmux_log).unwrap_or_default();
    let has_plugin_manager_env = log
        .lines()
        .any(|line| line.contains("set-environment") && line.contains("TMUX_PLUGIN_MANAGER_PATH"));
    let has_run_shell = log.lines().any(|line| line.split_whitespace().next() == Some("run-shell"));
    assert!(
        has_plugin_manager_env || has_run_shell,
        "expected loader activity (`set-environment ... TMUX_PLUGIN_MANAGER_PATH` or `run-shell`) \
after sync plugin failure, got log:\n{log}"
    );
}
