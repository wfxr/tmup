use std::fs;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

/// Clone a git repository into the staging directory.
pub async fn clone_repo(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new("git")
        .args(["clone", "--filter=blob:none", url])
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git clone")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed: {stderr}");
    }
    Ok(())
}

/// Fetch updates in an existing repository.
pub async fn fetch(repo: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["fetch", "--all", "--prune"])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git fetch")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git fetch failed: {stderr}");
    }
    Ok(())
}

/// Checkout a specific revision (commit, tag, or branch).
pub async fn checkout(repo: &Path, rev: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["checkout", rev])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git checkout")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git checkout {rev} failed: {stderr}");
    }
    Ok(())
}

/// Get the HEAD commit hash of a repository (synchronous).
pub fn head_commit_sync(repo: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to run git rev-parse HEAD")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-parse HEAD failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the HEAD commit hash of a repository.
pub async fn head_commit(repo: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git rev-parse HEAD")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-parse HEAD failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve the remote tracking branch to a commit.
pub async fn resolve_remote_branch(repo: &Path, branch: &str) -> Result<String> {
    let remote_ref = format!("origin/{branch}");
    let output = Command::new("git")
        .args(["rev-parse", &remote_ref])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        bail!("failed to resolve {remote_ref}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve the default branch name of the remote.
pub async fn default_branch(repo: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if output.status.success() {
        let full = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // refs/remotes/origin/main -> main
        if let Some(branch) = full.strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_string());
        }
    }
    // Fallback: try common names
    for name in &["main", "master"] {
        let check = Command::new("git")
            .args(["rev-parse", &format!("origin/{name}")])
            .current_dir(repo)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if check.success() {
            return Ok(name.to_string());
        }
    }
    bail!("cannot determine default branch")
}

/// Run a build command in the plugin directory.
pub fn run_build(dir: &Path, command: &str) -> Result<std::process::Output> {
    let output = std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run build: {command}"))?;
    Ok(output)
}

/// Publish protocol: fresh install (no existing target).
///
/// 1. rename(staging, target)
/// 2. run build in target (if provided)
/// 3. on build failure: remove target
pub fn publish_fresh_install(staging: &Path, target: &Path, build: Option<&str>) -> Result<()> {
    // Ensure parent directories exist
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(staging, target).with_context(|| {
        format!("failed to rename staging -> target: {} -> {}", staging.display(), target.display())
    })?;

    if let Some(cmd) = build {
        let output = run_build(target, cmd)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up failed target
            let _ = fs::remove_dir_all(target);
            bail!("build failed in {}: {stderr}", target.display());
        }
    }

    Ok(())
}

/// Publish protocol: replace existing plugin.
///
/// 1. rename(target, backup)
/// 2. rename(staging, target)
/// 3. run build in target (if provided)
/// 4. success: remove backup
/// 5. failure: remove failed target, rename(backup, target)
pub fn publish_replace(
    staging: &Path,
    target: &Path,
    backup: &Path,
    build: Option<&str>,
) -> Result<()> {
    // Ensure backup parent exists
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)?;
    }

    // Step 1: target -> backup
    fs::rename(target, backup).with_context(|| {
        format!("failed to backup: {} -> {}", target.display(), backup.display())
    })?;

    // Step 2: staging -> target
    if let Err(e) = fs::rename(staging, target) {
        // Rollback: restore backup
        let _ = fs::rename(backup, target);
        return Err(e).context("failed to publish staging to target");
    }

    // Step 3: run build
    if let Some(cmd) = build {
        let output = run_build(target, cmd)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Rollback: remove failed target, restore backup
            let _ = fs::remove_dir_all(target);
            let _ = fs::rename(backup, target);
            bail!("build failed, rolled back: {stderr}");
        }
    }

    // Success: remove backup
    let _ = fs::remove_dir_all(backup);

    Ok(())
}
