use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use kdl::KdlDocument;

use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};
use crate::state::validate_plugin_id;

/// Parse a KDL-formatted configuration string into a [`Config`].
pub fn parse_config(input: &str) -> Result<Config> {
    let doc: KdlDocument = input.parse().context("failed to parse KDL")?;

    let options = parse_options(&doc)?;
    let mut plugins = Vec::new();

    for node in doc.nodes() {
        if node.name().value() == "plugin" {
            let spec = parse_plugin(node)?;
            plugins.push(spec);
        }
    }

    validate_unique_ids(&plugins)?;

    Ok(Config { options, plugins })
}

fn parse_options(doc: &KdlDocument) -> Result<Options> {
    let mut opts = Options::default();

    let Some(node) = doc.get("options") else {
        return Ok(opts);
    };
    let Some(children) = node.children() else {
        return Ok(opts);
    };

    if let Some(v) = children.get_arg("auto-install") {
        opts.auto_install = v.as_bool().context("auto-install must be a bool")?;
    }

    Ok(opts)
}

fn parse_plugin(node: &kdl::KdlNode) -> Result<PluginSpec> {
    let raw = node
        .get(0)
        .and_then(|v| v.as_string())
        .context("plugin requires a source string as first argument")?
        .to_string();

    let is_local = get_bool(node, &raw, "local")?.unwrap_or(false);

    let explicit_name = get_string(node, &raw, "name")?;

    let opt_prefix = get_string(node, &raw, "opt-prefix")?.unwrap_or_default();

    let branch = get_string(node, &raw, "branch")?;
    let tag = get_string(node, &raw, "tag")?;
    let commit = get_string(node, &raw, "commit")?;

    let build = get_string(node, &raw, "build")?;

    // Parse tracking selector
    let selector_count = [&branch, &tag, &commit].iter().filter(|v| v.is_some()).count();
    ensure!(
        selector_count <= 1,
        "plugin \"{raw}\": branch, tag, commit are mutually exclusive (got {selector_count})"
    );

    let tracking = if let Some(b) = branch {
        Tracking::Branch(b)
    } else if let Some(t) = tag {
        Tracking::Tag(t)
    } else if let Some(c) = commit {
        Tracking::Commit(c)
    } else {
        Tracking::DefaultBranch
    };

    // Parse child nodes: opt entries and build (as child node)
    let mut opts = Vec::new();
    let mut child_build: Option<String> = None;
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "opt" => {
                    let key = child
                        .get(0)
                        .and_then(|v| v.as_string())
                        .context("opt requires a key string")?
                        .to_string();
                    let value = child
                        .get(1)
                        .and_then(|v| v.as_string())
                        .context("opt requires a value string")?
                        .to_string();
                    opts.push((key, value));
                }
                "build" => {
                    child_build = Some(
                        child
                            .get(0)
                            .and_then(|v| v.as_string())
                            .context("build child node requires a command string")?
                            .to_string(),
                    );
                }
                _ => {}
            }
        }
    }

    ensure!(
        !(build.is_some() && child_build.is_some()),
        "plugin \"{raw}\": build specified both as property and child node"
    );
    let build = build.or(child_build);

    let source = if is_local {
        let expanded_path = expand_local_path(&raw)?;
        ensure!(
            matches!(tracking, Tracking::DefaultBranch),
            "local plugin \"{raw}\": branch/tag/commit not allowed for local plugins"
        );
        ensure!(
            Path::new(&expanded_path).is_absolute(),
            "plugin \"{raw}\": local path must expand to an absolute path (got {expanded_path})"
        );
        let name = explicit_name.unwrap_or_else(|| {
            Path::new(&expanded_path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| expanded_path.clone())
        });
        PluginSpec {
            source: PluginSource::Local { path: expanded_path },
            name,
            opt_prefix,
            tracking,
            build,
            opts,
        }
    } else {
        let (id, clone_url) = normalize_remote_source(&raw)?;
        let name =
            explicit_name.unwrap_or_else(|| id.rsplit('/').next().unwrap_or(&id).to_string());
        PluginSpec {
            source: PluginSource::Remote { raw, id, clone_url },
            name,
            opt_prefix,
            tracking,
            build,
            opts,
        }
    };

    Ok(source)
}

/// Normalize a remote source string into (canonical_id, clone_url).
///
/// Rules:
/// - `user/repo` -> id: `github.com/user/repo`, url: `https://github.com/user/repo.git`
/// - `https://github.com/user/repo.git` -> id: `github.com/user/repo`, url as-is
/// - `git@github.com:user/repo.git` -> id: `github.com/user/repo`, url as-is
/// - Custom hosts preserved as-is
pub fn normalize_remote_source(raw: &str) -> Result<(String, String)> {
    // SSH URL: git@host:owner/repo.git
    if let Some(rest) = raw.strip_prefix("git@") {
        let (host, path) = rest.split_once(':').context("invalid SSH URL: missing ':'")?;
        let id = normalize_remote_id(host, path)?;
        return Ok((id, raw.to_string()));
    }

    // HTTPS/HTTP URL
    if raw.starts_with("https://") || raw.starts_with("http://") {
        let without_scheme =
            raw.strip_prefix("https://").or_else(|| raw.strip_prefix("http://")).unwrap();
        let (host, path) = without_scheme
            .split_once('/')
            .context("invalid remote URL: missing repository path")?;
        let id = normalize_remote_id(host, path)?;
        return Ok((id, raw.to_string()));
    }

    // GitHub shorthand: user/repo or org/repo
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        let id = format!("github.com/{raw}");
        validate_plugin_id(&id)?;
        let clone_url = format!("https://github.com/{raw}.git");
        return Ok((id, clone_url));
    }

    bail!("cannot parse remote source: \"{raw}\"")
}

fn normalize_remote_id(host: &str, path: &str) -> Result<String> {
    ensure!(
        !host.is_empty()
            && host != "."
            && host != ".."
            && !host.contains('/')
            && !host.contains('\\'),
        "unsafe remote host: {host:?}"
    );
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    ensure!(!path.is_empty(), "invalid remote URL: missing repository path");
    let id = format!("{host}/{path}");
    validate_plugin_id(&id)?;
    Ok(id)
}

fn validate_unique_ids(plugins: &[PluginSpec]) -> Result<()> {
    let mut seen = HashSet::new();
    for p in plugins {
        if let Some(id) = p.remote_id()
            && !seen.insert(id.to_string())
        {
            bail!("duplicate remote plugin id: \"{id}\"");
        }
    }
    Ok(())
}

/// Extract an optional string property, erroring if the property exists but is not a string.
fn get_string(node: &kdl::KdlNode, plugin: &str, key: &str) -> Result<Option<String>> {
    match node.get(key) {
        None => Ok(None),
        Some(v) => match v.as_string() {
            Some(s) => Ok(Some(s.to_string())),
            None => bail!("plugin \"{plugin}\": {key} must be a string"),
        },
    }
}

/// Extract an optional bool property, erroring if the property exists but is not a bool.
fn get_bool(node: &kdl::KdlNode, plugin: &str, key: &str) -> Result<Option<bool>> {
    match node.get(key) {
        None => Ok(None),
        Some(v) => match v.as_bool() {
            Some(b) => Ok(Some(b)),
            None => bail!("plugin \"{plugin}\": {key} must be a bool"),
        },
    }
}

fn expand_local_path(raw: &str) -> Result<String> {
    let expanded = shellexpand::full(raw)
        .with_context(|| format!("failed to expand local path: {raw}"))?
        .into_owned();
    Ok(expanded)
}
