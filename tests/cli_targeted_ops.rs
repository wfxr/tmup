mod utils;
use assert_cmd::Command;
use tempfile::tempdir;
use utils::*;

fn make_remote_repo(root: &std::path::Path) -> std::path::PathBuf {
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

fn write_config(root: &std::path::Path) -> std::path::PathBuf {
    let config_dir = root.join("config/tmux");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lazy.kdl");
    std::fs::write(
        &config_path,
        concat!(
            "plugin \"https://example.com/test/plugin.git\"\n",
            "plugin \"https://example.com/bad/plugin.git\"\n",
        ),
    )
    .unwrap();
    config_path
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

fn cargo_cmd(
    root: &std::path::Path,
    config_path: &std::path::Path,
    gitconfig: &std::path::Path,
) -> Command {
    let mut cmd = Command::cargo_bin("lazytmux").unwrap();
    cmd.env("LAZY_TMUX_CONFIG", config_path)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("HOME", root);
    cmd
}

#[test]
fn install_target_ignores_unrelated_sync_failures() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let config_path = write_config(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    cargo_cmd(dir.path(), &config_path, &gitconfig)
        .args(["install", "example.com/test/plugin"])
        .assert()
        .success();

    assert!(dir.path().join("data/lazytmux/plugins/example.com/test/plugin/init.tmux").exists());

    let lock = std::fs::read_to_string(dir.path().join("config/tmux/lazylock.json")).unwrap();
    assert!(lock.contains("example.com/test/plugin"));
    assert!(!lock.contains("example.com/bad/plugin"));
}

#[test]
fn update_target_ignores_unrelated_sync_failures() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let config_path = write_config(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    cargo_cmd(dir.path(), &config_path, &gitconfig)
        .args(["sync", "example.com/test/plugin"])
        .assert()
        .success();

    cargo_cmd(dir.path(), &config_path, &gitconfig)
        .args(["update", "example.com/test/plugin"])
        .assert()
        .success();

    let lock = std::fs::read_to_string(dir.path().join("config/tmux/lazylock.json")).unwrap();
    assert!(lock.contains("example.com/test/plugin"));
    assert!(!lock.contains("example.com/bad/plugin"));
}

#[test]
fn restore_target_ignores_unrelated_sync_failures() {
    let dir = tempdir().unwrap();
    make_remote_repo(dir.path());
    let config_path = write_config(dir.path());
    let gitconfig = write_git_rewrite_config(dir.path());

    cargo_cmd(dir.path(), &config_path, &gitconfig)
        .args(["sync", "example.com/test/plugin"])
        .assert()
        .success();

    cargo_cmd(dir.path(), &config_path, &gitconfig)
        .args(["restore", "example.com/test/plugin"])
        .assert()
        .success();

    let lock = std::fs::read_to_string(dir.path().join("config/tmux/lazylock.json")).unwrap();
    assert!(lock.contains("example.com/test/plugin"));
    assert!(!lock.contains("example.com/bad/plugin"));
}
