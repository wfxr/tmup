use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn sync_errors_on_unknown_plugin_id() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("tmup.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["sync", "github.com/user/other"])
        .env("TMUP_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown plugin id"));
}

#[test]
fn sync_errors_on_local_plugin_target() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("tmup.kdl");
    std::fs::write(&config_path, r#"plugin "/tmp/local-plugin" local=#true name="local-plugin""#)
        .unwrap();

    Command::cargo_bin("tmup")
        .unwrap()
        .args(["sync", "/tmp/local-plugin"])
        .env("TMUP_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown plugin id"));
}
