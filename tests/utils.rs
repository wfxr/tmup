#![allow(dead_code)]

use std::path::Path;

/// Run a hermetic git command in the given directory.
pub fn git(args: &[&str], dir: &Path) -> String {
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

/// Create a bare repo with one commit and return (bare_path, commit_hash).
pub fn make_bare_repo(root: &Path) -> (std::path::PathBuf, String) {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();

    git(&["init", "-b", "main"], &work);
    std::fs::write(work.join("init.tmux"), "#!/bin/sh\n").unwrap();
    git(&["add", "."], &work);
    git(&["commit", "-m", "init"], &work);

    let commit = git(&["rev-parse", "HEAD"], &work);

    let bare = root.join("bare.git");
    git(&["clone", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()], root);

    (bare, commit)
}

/// Add a commit to the default branch of a bare repo and push it.
pub fn push_commit(bare: &Path, message: &str) -> String {
    let tmp = bare.parent().unwrap().join(format!("_push_{message}_tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    git(&["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()], bare.parent().unwrap());
    std::fs::write(tmp.join(format!("{message}.txt")), message).unwrap();
    git(&["add", "."], &tmp);
    git(&["commit", "-m", message], &tmp);
    git(&["push"], &tmp);
    let hash = git(&["rev-parse", "HEAD"], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
    hash
}

/// Create a new branch on a bare repo, push a commit, and return its hash.
pub fn push_branch_commit(bare: &Path, branch: &str, message: &str) -> String {
    let tmp = bare.parent().unwrap().join(format!("_branch_{branch}_tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    git(&["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()], bare.parent().unwrap());
    git(&["checkout", "-b", branch], &tmp);
    std::fs::write(tmp.join(format!("{message}.txt")), message).unwrap();
    git(&["add", "."], &tmp);
    git(&["commit", "-m", message], &tmp);
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    git(&["push", "-u", "origin", &refspec], &tmp);
    let hash = git(&["rev-parse", "HEAD"], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
    hash
}

/// Tag a commit in a bare repo.
pub fn push_tag(bare: &Path, tag: &str, commit: &str) {
    let tmp = bare.parent().unwrap().join("_tag_tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    git(&["clone", bare.to_str().unwrap(), tmp.to_str().unwrap()], bare.parent().unwrap());
    git(&["tag", tag, commit], &tmp);
    git(&["push", "origin", tag], &tmp);
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// Clone a bare repo into a target directory (simulating an installed plugin).
pub fn clone_to_target(source: &Path, target: &Path) {
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    git(&["clone", source.to_str().unwrap(), target.to_str().unwrap()], target.parent().unwrap());
}
