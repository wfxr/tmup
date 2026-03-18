use lazytmux::{
    lockfile::{LockEntry, LockFile, TrackingRecord},
    model::{Config, Options, PluginSource, PluginSpec, Tracking},
    plugin,
    state::Paths,
};
use tempfile::tempdir;

/// Create a bare repo with one commit and return (bare_path, commit_hash).
fn make_bare_repo(root: &std::path::Path) -> (std::path::PathBuf, String) {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();

    let run = |args: &[&str], dir: &std::path::Path| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            // Hermetic: ignore system/global config, GPG signing, and hooks.
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
    };

    run(&["init", "-b", "main"], &work);
    std::fs::write(work.join("init.tmux"), "#!/bin/sh\n").unwrap();
    run(&["add", "."], &work);
    run(&["commit", "-m", "init"], &work);

    let commit = run(&["rev-parse", "HEAD"], &work);

    let bare = root.join("bare.git");
    run(
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

/// Build a Config with a single remote plugin pointing at a local bare repo.
fn make_config(clone_url: &str, build: Option<&str>) -> Config {
    Config {
        options: Options::default(),
        plugins: vec![PluginSpec {
            source:     PluginSource::Remote {
                raw:       "test/plugin".into(),
                id:        "example.com/test/plugin".into(),
                clone_url: clone_url.into(),
            },
            name:       "plugin".into(),
            opt_prefix: String::new(),
            tracking:   Tracking::DefaultBranch,
            build:      build.map(String::from),
            opts:       vec![],
        }],
    }
}

// ---------------------------------------------------------------------------
// Regression: same-commit restore must not replace build artifacts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_same_commit_preserves_build_artifacts() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    let cfg = make_config(&clone_url, Some("touch built.marker"));

    let mut lock = LockFile::new();
    lock.plugins
        .insert("example.com/test/plugin".into(), LockEntry {
            source:   "test/plugin".into(),
            tracking: TrackingRecord { kind: "branch".into(), value: "main".into() },
            commit:   commit.clone(),
        });

    // First restore: installs from scratch, build runs and creates marker.
    plugin::restore(&cfg, &lock, &paths, None).await.unwrap();

    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(
        target.join("built.marker").exists(),
        "build should have created marker"
    );

    // Second restore: same commit — must be a no-op.
    plugin::restore(&cfg, &lock, &paths, None).await.unwrap();
    assert!(
        target.join("built.marker").exists(),
        "same-commit restore must not replace the directory and lose build artifacts"
    );
}

// ---------------------------------------------------------------------------
// Regression: restore build failure must return Err (non-zero exit)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restore_build_failure_returns_error() {
    let dir = tempdir().unwrap();
    let (bare, commit) = make_bare_repo(&dir.path().join("repo"));

    let paths = Paths::for_test(dir.path().join("data"), dir.path().join("state"));
    paths.ensure_dirs().unwrap();

    let clone_url = format!("file://{}", bare.display());
    // Build command that always fails.
    let cfg = make_config(&clone_url, Some("exit 1"));

    let mut lock = LockFile::new();
    lock.plugins
        .insert("example.com/test/plugin".into(), LockEntry {
            source:   "test/plugin".into(),
            tracking: TrackingRecord { kind: "branch".into(), value: "main".into() },
            commit:   commit.clone(),
        });

    let result = plugin::restore(&cfg, &lock, &paths, None).await;
    assert!(
        result.is_err(),
        "restore must propagate build failure as Err"
    );

    // The target should have been rolled back / removed by publish protocol.
    let target = paths.plugin_dir("example.com/test/plugin");
    assert!(
        !target.exists(),
        "failed fresh-install target should be cleaned up"
    );

    // A failure marker should have been written.
    let markers = lazytmux::state::read_failure_markers(&paths.failures_root).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].plugin_id, "example.com/test/plugin");
}
