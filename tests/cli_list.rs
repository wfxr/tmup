use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn list_prints_state_and_last_result_columns() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(data_dir.join("plugins/github.com/user/repo/.git")).unwrap();

    // Write config
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    // Write lockfile
    let lock_path = config_dir.join("lazylock.json");
    std::fs::write(
        &lock_path,
        r#"{"version":2,"plugins":{"github.com/user/repo":{"source":"user/repo","tracking":{"type":"branch","value":"main"},"commit":"abc1234"}}}"#,
    ).unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_DATA_HOME", dir.path().join("data").parent().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("state"))
        .stdout(predicate::str::contains("last-result"));
}

#[test]
fn list_shows_plugin_entries() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(
        &config_path,
        r#"
plugin "tmux-plugins/tmux-sensible"
plugin "catppuccin/tmux"
    "#,
    )
    .unwrap();

    // Write empty lockfile
    let lock_path = config_dir.join("lazylock.json");
    std::fs::write(&lock_path, r#"{"version":2,"plugins":{}}"#).unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux-sensible"))
        .stdout(predicate::str::contains("tmux"));
}

#[test]
fn list_warns_before_table_when_lock_metadata_is_stale() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    let data_home = dir.path().join("data");
    let state_home = dir.path().join("state");
    std::fs::create_dir_all(&config_dir).unwrap();

    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo" build="make install""#).unwrap();

    let lock_path = config_dir.join("lazylock.json");
    let stale_lock = r#"{"version":2,"plugins":{"github.com/user/repo":{"source":"user/repo","tracking":{"type":"branch","value":"main"},"commit":"abc1234"}}}"#;
    std::fs::write(&lock_path, stale_lock).unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "warning: lock metadata is stale relative to config; run `lazytmux sync`\n",
        ))
        .stdout(predicate::str::contains("state"))
        .stdout(predicate::str::contains("github.com/user/repo"));
}

#[test]
fn list_does_not_mutate_stale_lockfile() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    std::fs::create_dir_all(&config_dir).unwrap();

    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, r#"plugin "user/repo""#).unwrap();

    let lock_path = config_dir.join("lazylock.json");
    let original = r#"{"version":2,"plugins":{"github.com/user/repo":{"source":"user/repo","tracking":{"type":"branch","value":"main"},"commit":"abc1234"}}}"#;
    std::fs::write(&lock_path, original).unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .assert()
        .success();

    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), original);
}

#[test]
fn list_marks_missing_local_plugin_as_missing() {
    let dir = tempdir().unwrap();
    let config_home = dir.path().join("config");
    let config_dir = config_home.join("tmux");
    let data_home = dir.path().join("data");
    let state_home = dir.path().join("state");
    std::fs::create_dir_all(&config_dir).unwrap();

    let missing_local = dir.path().join("missing-plugin");
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(&config_path, format!(r#"plugin "{}" local=#true"#, missing_local.display()))
        .unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .assert()
        .success()
        .stdout(predicate::str::contains(missing_local.to_string_lossy().as_ref()))
        .stdout(predicate::str::contains("missing"))
        .stdout(predicate::str::contains("none"));
}
