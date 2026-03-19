use lazytmux::{
    lockfile::{LockFile, config_fingerprint, remote_plugin_config_hash},
    model::{Config, Options, PluginSource, PluginSpec, Tracking},
    state::{Paths, build_command_hash},
    sync::{self, SyncPolicy},
};
use tempfile::tempdir;

fn git(args: &[&str], dir: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn make_bare_repo(root: &std::path::Path) -> (std::path::PathBuf, String) {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();

    git(&["init", "-b", "main"], &work);
    std::fs::write(work.join("init.tmux"), "#!/bin/sh\n").unwrap();
    git(&["add", "."], &work);
    git(&["commit", "-m", "init"], &work);

    let commit = git(&["rev-parse", "HEAD"], &work);

    let bare = root.join("bare.git");
    git(
        &[
            "clone",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
        root,
    );

    (bare, commit)
}

fn push_commit(bare: &std::path::Path, message: &str) -> String {
    let tmp = bare.parent().unwrap().join(format!("_push_{message}_tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    git(
        &["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()],
        bare.parent().unwrap(),
    );
    std::fs::write(tmp.join(format!("{message}.txt")), message).unwrap();
    git(&["add", "."], &tmp);
    git(&["commit", "-m", message], &tmp);
    git(&["push"], &tmp);
    let hash = git(&["rev-parse", "HEAD"], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
    hash
}

fn push_branch_commit(bare: &std::path::Path, branch: &str, message: &str) -> String {
    let tmp = bare.parent().unwrap().join(format!("_branch_{branch}_tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    git(
        &["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()],
        bare.parent().unwrap(),
    );
    git(&["checkout", "-b", branch], &tmp);
    std::fs::write(tmp.join(format!("{message}.txt")), message).unwrap();
    git(&["add", "."], &tmp);
    git(&["commit", "-m", message], &tmp);
    git(&["push", "-u", "origin", branch], &tmp);
    let hash = git(&["rev-parse", "HEAD"], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
    hash
}

fn push_tag(bare: &std::path::Path, tag: &str, commit: &str) {
    let tmp = bare.parent().unwrap().join("_tag_tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    git(
        &["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()],
        bare.parent().unwrap(),
    );
    git(&["tag", tag, commit], &tmp);
    git(&["push", "origin", tag], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
}

fn make_plugin(
    raw: &str,
    id: &str,
    clone_url: &str,
    tracking: Tracking,
    build: Option<&str>,
) -> PluginSpec {
    PluginSpec {
        source: PluginSource::Remote {
            raw:       raw.into(),
            id:        id.into(),
            clone_url: clone_url.into(),
        },
        name: raw.rsplit('/').next().unwrap_or(raw).into(),
        opt_prefix: String::new(),
        tracking,
        build: build.map(String::from),
        opts: vec![],
    }
}

fn make_config(plugins: Vec<PluginSpec>) -> Config {
    Config { options: Options::default(), plugins }
}

fn plugin_head(paths: &Paths, id: &str) -> String {
    git(&["rev-parse", "HEAD"], &paths.plugin_dir(id))
}

#[tokio::test]
async fn sync_installs_new_remote_plugin_and_persists_metadata() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let plugin = make_plugin(
        "test/plugin",
        "example.com/test/plugin",
        &clone_url,
        Tracking::DefaultBranch,
        Some("touch built.marker"),
    );
    let cfg = make_config(vec![plugin.clone()]);
    let mut lock = LockFile::new();

    sync::run(&cfg, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    let expected_config_hash = remote_plugin_config_hash(&plugin).unwrap();
    let expected_fingerprint = config_fingerprint(&cfg);
    assert_eq!(entry.commit, commit);
    assert_eq!(entry.tracking.kind, "default-branch");
    assert_eq!(entry.tracking.value, "main");
    assert_eq!(
        entry.config_hash.as_deref(),
        Some(expected_config_hash.as_str())
    );
    assert_eq!(
        lock.config_fingerprint.as_deref(),
        Some(expected_fingerprint.as_str())
    );
    assert_eq!(plugin_head(&paths, "example.com/test/plugin"), commit);
    assert!(
        paths
            .plugin_dir("example.com/test/plugin")
            .join("built.marker")
            .exists()
    );
}

#[tokio::test]
async fn sync_reconciles_branch_to_tag_and_commit_transitions() {
    let dir = tempdir().unwrap();
    let (bare, commit_a) = make_bare_repo(&dir.path().join("repo"));
    let commit_b = push_commit(&bare, "second");
    push_tag(&bare, "v1.0.0", &commit_a);

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let mut lock = LockFile::new();

    let branch_cfg = make_config(vec![make_plugin(
        "test/plugin",
        "example.com/test/plugin",
        &clone_url,
        Tracking::Branch("main".into()),
        None,
    )]);
    sync::run(&branch_cfg, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();
    assert_eq!(plugin_head(&paths, "example.com/test/plugin"), commit_b);

    let tag_cfg = make_config(vec![make_plugin(
        "test/plugin",
        "example.com/test/plugin",
        &clone_url,
        Tracking::Tag("v1.0.0".into()),
        None,
    )]);
    sync::run(&tag_cfg, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();
    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.tracking.kind, "tag");
    assert_eq!(entry.tracking.value, "v1.0.0");
    assert_eq!(entry.commit, commit_a);
    assert_eq!(plugin_head(&paths, "example.com/test/plugin"), commit_a);

    let commit_cfg = make_config(vec![make_plugin(
        "test/plugin",
        "example.com/test/plugin",
        &clone_url,
        Tracking::Commit(commit_b.clone()),
        None,
    )]);
    sync::run(&commit_cfg, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();
    let entry = lock.plugins.get("example.com/test/plugin").unwrap();
    assert_eq!(entry.tracking.kind, "commit");
    assert_eq!(entry.tracking.value, commit_b);
    assert_eq!(entry.commit, entry.tracking.value);
    assert_eq!(plugin_head(&paths, "example.com/test/plugin"), entry.commit);
}

#[tokio::test]
async fn sync_updates_only_the_targeted_plugin_id() {
    let dir = tempdir().unwrap();
    let (bare_a, commit_a) = make_bare_repo(&dir.path().join("repo-a"));
    let (bare_b, commit_b_main) = make_bare_repo(&dir.path().join("repo-b"));
    let commit_b_feature = push_branch_commit(&bare_b, "feature", "feature");

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_a = format!("file://{}", bare_a.display());
    let clone_b = format!("file://{}", bare_b.display());

    let cfg_initial = make_config(vec![
        make_plugin(
            "test/plugin-a",
            "example.com/test/plugin-a",
            &clone_a,
            Tracking::Branch("main".into()),
            None,
        ),
        make_plugin(
            "test/plugin-b",
            "example.com/test/plugin-b",
            &clone_b,
            Tracking::Branch("main".into()),
            None,
        ),
    ]);
    let mut lock = LockFile::new();
    sync::run(&cfg_initial, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    assert_eq!(plugin_head(&paths, "example.com/test/plugin-a"), commit_a);
    assert_eq!(
        plugin_head(&paths, "example.com/test/plugin-b"),
        commit_b_main
    );

    let original_a_hash = lock.plugins["example.com/test/plugin-a"]
        .config_hash
        .clone();

    let cfg_changed = make_config(vec![
        make_plugin(
            "test/plugin-a",
            "example.com/test/plugin-a",
            &clone_a,
            Tracking::Branch("feature".into()),
            None,
        ),
        make_plugin(
            "test/plugin-b",
            "example.com/test/plugin-b",
            &clone_b,
            Tracking::Branch("feature".into()),
            None,
        ),
    ]);

    sync::run(
        &cfg_changed,
        &mut lock,
        &paths,
        Some("example.com/test/plugin-b"),
        SyncPolicy::SYNC,
    )
    .await
    .unwrap();

    assert_eq!(plugin_head(&paths, "example.com/test/plugin-a"), commit_a);
    assert_eq!(
        plugin_head(&paths, "example.com/test/plugin-b"),
        commit_b_feature
    );
    assert_eq!(
        lock.plugins["example.com/test/plugin-a"].config_hash,
        original_a_hash
    );
    assert_eq!(lock.plugins["example.com/test/plugin-a"].commit, commit_a);
    assert_eq!(
        lock.plugins["example.com/test/plugin-b"].commit,
        commit_b_feature
    );
}

#[tokio::test]
async fn sync_prunes_removed_lock_entries_without_deleting_repo_dirs() {
    let dir = tempdir().unwrap();
    let (bare_a, _commit_a) = make_bare_repo(&dir.path().join("repo-a"));
    let (bare_b, _commit_b) = make_bare_repo(&dir.path().join("repo-b"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_a = format!("file://{}", bare_a.display());
    let clone_b = format!("file://{}", bare_b.display());
    let mut lock = LockFile::new();

    let cfg_initial = make_config(vec![
        make_plugin(
            "test/plugin-a",
            "example.com/test/plugin-a",
            &clone_a,
            Tracking::DefaultBranch,
            None,
        ),
        make_plugin(
            "test/plugin-b",
            "example.com/test/plugin-b",
            &clone_b,
            Tracking::DefaultBranch,
            None,
        ),
    ]);
    sync::run(&cfg_initial, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    let removed_dir = paths.plugin_dir("example.com/test/plugin-b");
    assert!(removed_dir.exists());

    let cfg_removed = make_config(vec![make_plugin(
        "test/plugin-a",
        "example.com/test/plugin-a",
        &clone_a,
        Tracking::DefaultBranch,
        None,
    )]);
    sync::run(&cfg_removed, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    assert!(lock.plugins.contains_key("example.com/test/plugin-a"));
    assert!(!lock.plugins.contains_key("example.com/test/plugin-b"));
    assert!(
        removed_dir.exists(),
        "sync should not delete repo directories"
    );
}

#[tokio::test]
async fn sync_rebuilds_same_commit_when_only_build_changes_and_rewrites_markers() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let plugin_id = "example.com/test/plugin";
    let cfg_initial = make_config(vec![make_plugin(
        "test/plugin",
        plugin_id,
        &clone_url,
        Tracking::DefaultBranch,
        Some("touch built-v1.marker"),
    )]);
    let mut lock = LockFile::new();
    sync::run(&cfg_initial, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    let target = paths.plugin_dir(plugin_id);
    assert!(target.join("built-v1.marker").exists());

    let cfg_fail = make_config(vec![make_plugin(
        "test/plugin",
        plugin_id,
        &clone_url,
        Tracking::DefaultBranch,
        Some("touch built-v2.marker; exit 1"),
    )]);
    let result = sync::run(&cfg_fail, &mut lock, &paths, None, SyncPolicy::SYNC).await;
    assert!(result.is_err(), "failing rebuild should return Err");
    assert_eq!(plugin_head(&paths, plugin_id), commit);
    assert!(target.join("built-v1.marker").exists());
    assert!(!target.join("built-v2.marker").exists());

    let fail_hash = build_command_hash("touch built-v2.marker; exit 1");
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].plugin_id, plugin_id);
    assert_eq!(markers[0].commit, commit);
    assert_eq!(markers[0].build_hash, fail_hash);

    let previous_hash = lock.plugins[plugin_id].config_hash.clone();

    let cfg_success = make_config(vec![make_plugin(
        "test/plugin",
        plugin_id,
        &clone_url,
        Tracking::DefaultBranch,
        Some("touch built-v2.marker"),
    )]);
    sync::run(&cfg_success, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    assert_eq!(plugin_head(&paths, plugin_id), commit);
    assert!(!target.join("built-v1.marker").exists());
    assert!(target.join("built-v2.marker").exists());
    assert!(
        lazytmux::state::read_failure_markers(&paths.failures_root)
            .unwrap()
            .is_empty()
    );
    assert_ne!(lock.plugins[plugin_id].config_hash, previous_hash);
}

#[tokio::test]
async fn sync_rebuilds_when_build_changes_and_tracking_changes_to_same_commit() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));
    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let plugin_id = "example.com/test/plugin";

    let cfg_initial = make_config(vec![make_plugin(
        "test/plugin",
        plugin_id,
        &clone_url,
        Tracking::DefaultBranch,
        Some("touch built-v1.marker"),
    )]);
    let mut lock = LockFile::new();
    sync::run(&cfg_initial, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    let target = paths.plugin_dir(plugin_id);
    assert_eq!(plugin_head(&paths, plugin_id), commit);
    assert!(target.join("built-v1.marker").exists());

    let cfg_changed = make_config(vec![make_plugin(
        "test/plugin",
        plugin_id,
        &clone_url,
        Tracking::Branch("main".into()),
        Some("touch built-v2.marker"),
    )]);
    sync::run(&cfg_changed, &mut lock, &paths, None, SyncPolicy::SYNC)
        .await
        .unwrap();

    let entry = lock.plugins.get(plugin_id).unwrap();
    assert_eq!(entry.commit, commit);
    assert_eq!(entry.tracking.kind, "branch");
    assert_eq!(entry.tracking.value, "main");
    assert!(!target.join("built-v1.marker").exists());
    assert!(target.join("built-v2.marker").exists());
}
