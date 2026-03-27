use std::fs;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

/// Object filter applied to clone/fetch operations when the remote supports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFilter {
    /// Delay blob transfer until content is needed.
    BlobNone,
}

impl ObjectFilter {
    fn as_filter_spec(self) -> &'static str {
        match self {
            Self::BlobNone => "blob:none",
        }
    }
}

/// Clone a bare git repository into the cache directory.
pub async fn clone_bare_repo(url: &str, dest: &Path) -> Result<()> {
    clone_bare_repo_filtered(url, dest, None).await
}

/// Clone a bare git repository into the cache directory with an optional filter.
pub async fn clone_bare_repo_filtered(
    url: &str,
    dest: &Path,
    filter: Option<ObjectFilter>,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut command = Command::new("git");
    command.args(["clone", "--bare"]);
    if let Some(filter) = filter {
        command.arg(format!("--filter={}", filter.as_filter_spec()));
    }
    let output = command
        .arg(url)
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git clone --bare")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone --bare failed: {stderr}");
    }
    Ok(())
}

/// Clone a local repository into a working staging directory without hardlinks.
pub async fn clone_local_repo(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let output = Command::new("git")
        .args(["clone", "--local", "--no-hardlinks"])
        .arg(source)
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git clone --local --no-hardlinks")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone --local --no-hardlinks failed: {stderr}");
    }
    Ok(())
}

/// Fetch updates from origin with the provided refspecs.
pub async fn fetch_origin(repo: &Path, refspecs: &[String]) -> Result<()> {
    fetch_origin_filtered(repo, refspecs, None).await
}

/// Fetch updates from origin with the provided refspecs and optional object filter.
pub async fn fetch_origin_filtered(
    repo: &Path,
    refspecs: &[String],
    filter: Option<ObjectFilter>,
) -> Result<()> {
    let mut command = Command::new("git");
    command.args(["fetch", "origin", "--prune", "--force"]);
    if let Some(filter) = filter {
        command.arg(format!("--filter={}", filter.as_filter_spec()));
    }
    command.args(refspecs);
    let output = command
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git fetch origin")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git fetch origin failed: {stderr}");
    }
    Ok(())
}

/// Update the remote HEAD reference to point at its default branch.
pub async fn set_remote_head(repo: &Path, remote: &str) -> Result<()> {
    // This is a best-effort hint for default-branch resolution. Some remotes can
    // still advertise fetchable branch refs while failing `set-head --auto`; in
    // that case callers fall back to probing common branch names.
    let _output = Command::new("git")
        .args(["remote", "set-head", remote, "--auto"])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git remote set-head")?;
    Ok(())
}

/// Set the URL for a named remote.
pub async fn set_remote_url(repo: &Path, remote: &str, url: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["remote", "set-url", remote, url])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("failed to run git remote set-url")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git remote set-url failed: {stderr}");
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

/// Return whether the given revision resolves to a commit in this repository.
pub async fn has_commit(repo: &Path, rev: &str) -> Result<bool> {
    let verify = format!("{rev}^{{commit}}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &verify])
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .context("failed to run git rev-parse --verify")?;
    Ok(output.status.success())
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

/// Resolve a tag ref to the commit it points at, even if a branch shares the same name.
pub async fn resolve_tag(repo: &Path, tag: &str) -> Result<String> {
    let tag_ref = format!("refs/tags/{tag}^{{}}");
    let output = Command::new("git")
        .args(["rev-parse", &tag_ref])
        .current_dir(repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to resolve tag {tag}: {stderr}");
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

/// Publish protocol: fresh install.
fn finalize_publish(staging: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(staging, target).with_context(|| {
        format!("failed to rename staging -> target: {} -> {}", staging.display(), target.display())
    })?;
    Ok(())
}

/// Publish protocol: fresh install (build runs in staging before swap).
pub fn publish_fresh_install(staging: &Path, target: &Path, build: Option<&str>) -> Result<()> {
    if let Some(cmd) = build {
        let output = run_build(staging, cmd)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("build failed in {}: {stderr}", staging.display());
        }
    }
    finalize_publish(staging, target)
}

/// Publish protocol: replace existing plugin.
///
/// Build runs in staging before touching the live target.
pub fn publish_replace(staging: &Path, target: &Path, build: Option<&str>) -> Result<()> {
    if let Some(cmd) = build {
        let output = run_build(staging, cmd)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("build failed in {}: {stderr}", staging.display());
        }
    }

    if target.exists() {
        fs::remove_dir_all(target)
            .with_context(|| format!("failed to remove existing target: {}", target.display()))?;
    }
    finalize_publish(staging, target)
}
