use anyhow::{Context, Result, bail, ensure};
use kdl::KdlDocument;
use std::collections::HashSet;

use crate::model::{Config, Options, PluginSource, PluginSpec, Tracking};

pub fn parse_config(input: &str) -> Result<Config> {
    let doc: KdlDocument = input.parse().context("failed to parse KDL")?;

    let options = parse_options(&doc)?;
    let mut plugins = Vec::new();

    for node in doc.nodes() {
        if node.name().to_string() == "plugin" {
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

    if let Some(v) = children.get_arg("concurrency") {
        opts.concurrency = v.as_integer().context("concurrency must be an integer")? as usize;
    }
    if let Some(v) = children.get_arg("auto-install") {
        opts.auto_install = v.as_bool().context("auto-install must be a bool")?;
    }
    if let Some(v) = children.get_arg("auto-clean") {
        opts.auto_clean = v.as_bool().context("auto-clean must be a bool")?;
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
    let selector_count = [&branch, &tag, &commit]
        .iter()
        .filter(|v| v.is_some())
        .count();
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

    // Parse child opt nodes
    let mut opts = Vec::new();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().to_string() == "opt" {
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
            if child.name().to_string() == "build" {
                // build can also be a child node
                // but we already handle it as a property; skip
            }
        }
    }

    // Also allow build as a child node
    if build.is_none()
        && let Some(children) = node.children()
        && let Some(build_arg) = children.get_arg("build")
    {
        let _ = build_arg; // already handled above via property
    }

    let build = build.or_else(|| {
        node.children()
            .and_then(|c| c.get_arg("build"))
            .and_then(|v| v.as_string())
            .map(String::from)
    });

    let source = if is_local {
        ensure!(
            matches!(tracking, Tracking::DefaultBranch),
            "local plugin \"{raw}\": branch/tag/commit not allowed for local plugins"
        );
        let name = explicit_name.unwrap_or_else(|| {
            std::path::Path::new(&raw)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| raw.clone())
        });
        PluginSpec {
            source: PluginSource::Local { path: raw },
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
        let (host, path) = rest
            .split_once(':')
            .context("invalid SSH URL: missing ':'")?;
        let path = path.strip_suffix(".git").unwrap_or(path);
        let id = format!("{host}/{path}");
        return Ok((id, raw.to_string()));
    }

    // HTTPS/HTTP URL
    if raw.starts_with("https://") || raw.starts_with("http://") {
        let without_scheme = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
            .unwrap();
        let id = without_scheme
            .strip_suffix(".git")
            .unwrap_or(without_scheme);
        return Ok((id.to_string(), raw.to_string()));
    }

    // GitHub shorthand: user/repo or org/repo
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        let id = format!("github.com/{raw}");
        let clone_url = format!("https://github.com/{raw}.git");
        return Ok((id, clone_url));
    }

    bail!("cannot parse remote source: \"{raw}\"")
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
