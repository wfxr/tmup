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
        r#"{"version":1,"plugins":{"github.com/user/repo":{"source":"user/repo","tracking":{"type":"branch","value":"main"},"commit":"abc1234"}}}"#,
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
    std::fs::write(&lock_path, r#"{"version":1,"plugins":{}}"#).unwrap();

    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .env("LAZY_TMUX_CONFIG", &config_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("tmux-sensible"))
        .stdout(predicate::str::contains("tmux"));
}
