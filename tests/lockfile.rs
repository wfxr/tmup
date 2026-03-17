use lazytmux::lockfile::{LockEntry, LockFile, read_lockfile, write_lockfile_atomic};
use tempfile::tempdir;

#[test]
fn round_trips_lockfile_json() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazylock.json");
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/tmux-plugins/tmux-sensible".into(),
        LockEntry::branch(
            "tmux-plugins/tmux-sensible",
            "main",
            "abc1234567890abcdef1234567890abcdef1234",
        ),
    );

    write_lockfile_atomic(&path, &lock).unwrap();
    let reread = read_lockfile(&path).unwrap();
    assert_eq!(reread.version, 1);
    assert_eq!(reread.plugins.len(), 1);
    let entry = &reread.plugins["github.com/tmux-plugins/tmux-sensible"];
    assert_eq!(entry.source, "tmux-plugins/tmux-sensible");
    assert_eq!(entry.tracking.kind, "branch");
    assert_eq!(entry.tracking.value, "main");
    assert_eq!(entry.commit, "abc1234567890abcdef1234567890abcdef1234");
}

#[test]
fn round_trips_multiple_plugins() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazylock.json");
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/tmux-plugins/tmux-sensible".into(),
        LockEntry::branch("tmux-plugins/tmux-sensible", "main", "aaa111"),
    );
    lock.plugins.insert(
        "github.com/catppuccin/tmux".into(),
        LockEntry::tag("catppuccin/tmux", "v1.0", "bbb222"),
    );
    lock.plugins.insert(
        "github.com/user/pinned".into(),
        LockEntry::commit("user/pinned", "ccc333"),
    );

    write_lockfile_atomic(&path, &lock).unwrap();
    let reread = read_lockfile(&path).unwrap();
    assert_eq!(reread.plugins.len(), 3);
    assert_eq!(
        reread.plugins["github.com/catppuccin/tmux"].tracking.kind,
        "tag"
    );
    assert_eq!(
        reread.plugins["github.com/user/pinned"].tracking.kind,
        "commit"
    );
}

#[test]
fn read_nonexistent_returns_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nonexistent.json");
    assert!(read_lockfile(&path).is_err());
}

#[test]
fn plugins_are_sorted_by_key() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazylock.json");
    let mut lock = LockFile::new();
    lock.plugins.insert(
        "github.com/z/z".into(),
        LockEntry::branch("z/z", "main", "aaa"),
    );
    lock.plugins.insert(
        "github.com/a/a".into(),
        LockEntry::branch("a/a", "main", "bbb"),
    );

    write_lockfile_atomic(&path, &lock).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    let pos_a = content.find("github.com/a/a").unwrap();
    let pos_z = content.find("github.com/z/z").unwrap();
    assert!(
        pos_a < pos_z,
        "plugins should be sorted alphabetically in JSON"
    );
}
