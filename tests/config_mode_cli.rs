mod utils;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;
use utils::{git, push_branch_commit};

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn make_remote_repo(root: &std::path::Path) {
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
    push_branch_commit(&bare, "feature", "feature");
}

fn write_git_rewrite_config(root: &std::path::Path) -> std::path::PathBuf {
    let gitconfig = root.join("gitconfig");
    let rewritten_base = format!("file://{}/", root.join("remotes/example.com").display());
    std::fs::write(
        &gitconfig,
        format!("[url \"{rewritten_base}\"]\n    insteadOf = https://example.com/\n"),
    )
    .unwrap();
    gitconfig
}

#[test]
fn config_mode_cli_list_mixed_reads_tpm_config_without_scaffolding_tmup_kdl() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    write_file(&config_dir.join("tmux.conf"), "set -g @plugin 'tmux-plugins/tmux-sensible'\n");

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["list", "--config-mode=mixed"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("HOME", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux-plugins/tmux-sensible"));

    assert!(
        !config_dir.join("tmup.kdl").exists(),
        "list should stay read-only and avoid scaffolding a missing tmup.kdl"
    );
}

#[test]
fn config_mode_cli_tmup_list_does_not_auto_create_missing_kdl() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    std::fs::create_dir_all(&config_dir).unwrap();

    Command::cargo_bin("tmup")
        .unwrap()
        .arg("list")
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("HOME", dir.path())
        .assert()
        .success();

    assert!(!config_dir.join("tmup.kdl").exists(), "list should not create a default tmup.kdl");
}

#[test]
fn config_mode_cli_list_mixed_does_not_auto_create_missing_tmup_config_override() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    write_file(&config_dir.join("tmux.conf"), "set -g @plugin 'tmux-plugins/tmux-sensible'\n");
    let override_dir = dir.path().join("override");
    let override_kdl = override_dir.join("missing.kdl");

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["list", "--config-mode=mixed"])
        .env("TMUP_CONFIG", &override_kdl)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("HOME", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux-plugins/tmux-sensible"));

    assert!(!override_kdl.exists(), "list should not create a missing TMUP_CONFIG path");
    let lock = override_dir.join("tmup.lock");
    assert!(!lock.exists(), "list should not create a lockfile");
}

#[test]
fn config_mode_cli_mixed_works_with_absolute_xdg_without_home() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    write_file(&config_dir.join("tmux.conf"), "set -g @plugin 'tmux-plugins/tmux-sensible'\n");

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["list", "--config-mode=mixed"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env_remove("HOME")
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux-plugins/tmux-sensible"));
}

#[test]
fn config_mode_cli_mixed_without_tpm_config_still_works_without_home() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::fs::create_dir_all(config_home.join("tmux")).unwrap();

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["list", "--config-mode=mixed"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env_remove("HOME")
        .assert()
        .success()
        .stdout(predicate::str::contains("Plugin"));
}

#[test]
fn config_mode_cli_list_mixed_warns_and_prefers_kdl() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    write_file(
        &config_dir.join("tmup.kdl"),
        r#"plugin "tmux-plugins/tmux-sensible" branch="feature""#,
    );
    write_file(
        &config_dir.join("tmux.conf"),
        concat!(
            "set -g @plugin 'tmux-plugins/tmux-sensible'\n",
            "set -g @plugin 'tmux-plugins/tmux-yank'\n",
        ),
    );
    write_file(&config_dir.join("tmup.lock"), r#"{"version":2,"plugins":{}}"#);

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["list", "--config-mode=mixed"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("HOME", dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: plugin \"github.com/tmux-plugins/tmux-sensible\" declared in both tmup.kdl and TPM config; using tmup.kdl entry",
        ))
        .stdout(predicate::str::contains("tmux-plugins/tmux-sensible"))
        .stdout(predicate::str::contains("tmux-plugins/tmux-yank"));
}

#[test]
fn config_mode_cli_sync_mixed_writes_lockfile_next_to_kdl_with_kdl_precedence() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    write_file(
        &config_dir.join("tmup.kdl"),
        r#"plugin "https://example.com/test/plugin.git" branch="feature""#,
    );
    write_file(
        &config_dir.join("tmux.conf"),
        "set -g @plugin 'https://example.com/test/plugin.git'\n",
    );

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["sync", "--config-mode=mixed"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &gitconfig)
        .env("HOME", dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: plugin \"example.com/test/plugin\" declared in both tmup.kdl and TPM config; using tmup.kdl entry",
        ));

    let lock = std::fs::read_to_string(config_dir.join("tmup.lock")).unwrap();
    assert!(lock.contains(r#""type": "branch""#), "{lock}");
    assert!(lock.contains(r#""value": "feature""#), "{lock}");
}
