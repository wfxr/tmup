use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::build_remote_plugin_spec;
use crate::model::{Config, Options, PluginSpec, Tracking};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourcedFile {
    path: String,
    quiet: bool,
}

/// Resolve the default TPM-style tmux config path from the supported search order.
pub fn resolve_config_path() -> Result<PathBuf> {
    if let Ok(xdg_config_home) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(xdg_config_home).join("tmux/tmux.conf");
        if path.exists() {
            return Ok(path);
        }
    }

    let config_home = home_dir().join(".config/tmux/tmux.conf");
    if config_home.exists() {
        return Ok(config_home);
    }

    let legacy_home = home_dir().join(".tmux.conf");
    if legacy_home.exists() {
        return Ok(legacy_home);
    }

    bail!("tmux config file not found")
}

/// Load plugin declarations from a TPM-style tmux config file.
pub fn load_config_from_path(path: &Path) -> Result<Config> {
    let mut plugins = Vec::new();
    let mut seen_remote_ids = std::collections::HashSet::new();
    for (source_path, content) in read_scan_inputs(path)? {
        for (lineno, line) in content.lines().enumerate() {
            let Some(raw) = plugin_declaration(line) else {
                continue;
            };
            let spec = parse_plugin_spec(&raw).with_context(|| {
                format!(
                    "failed to parse @plugin declaration in {}:{}",
                    source_path.display(),
                    lineno + 1
                )
            })?;
            let Some(id) = spec.remote_id() else {
                continue;
            };
            if seen_remote_ids.insert(id.to_string()) {
                plugins.push(spec);
            }
        }
    }

    Ok(Config { options: Options::default(), plugins })
}

fn read_scan_inputs(path: &Path) -> Result<Vec<(PathBuf, String)>> {
    let root = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read tmux config: {}", path.display()))?;
    let mut inputs = vec![(path.to_path_buf(), root.clone())];

    // Intentionally only scans direct sourced files discovered from the root tmux config,
    // matching current TPM behavior instead of recursively expanding nested includes.
    for sourced in direct_sourced_files(&root) {
        let sourced_path = expand_source_path(&sourced.path);
        let content = match std::fs::read_to_string(&sourced_path) {
            Ok(content) => content,
            Err(err) if sourced.quiet && err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to read sourced tmux config: {}", sourced_path.display())
                });
            }
        };
        inputs.push((sourced_path, content));
    }

    Ok(inputs)
}

fn direct_sourced_files(input: &str) -> Vec<SourcedFile> {
    input.lines().filter_map(source_directive).collect()
}

fn source_directive(line: &str) -> Option<SourcedFile> {
    let tokens = tokenize_tmux_line(line);
    if tokens.len() < 2 {
        return None;
    }

    match tokens[0].as_str() {
        "source" | "source-file" => {
            let (quiet, path_index) = match tokens.get(1) {
                Some(flag) if is_quiet_source_flag(flag) => (true, 2),
                Some(_) => (false, 1),
                None => return None,
            };
            tokens.get(path_index).cloned().map(|path| SourcedFile { path, quiet })
        }
        _ => None,
    }
}

fn is_quiet_source_flag(token: &str) -> bool {
    token.starts_with('-') && token[1..].chars().all(|ch| ch == 'q') && token.len() > 1
}

fn plugin_declaration(line: &str) -> Option<String> {
    let tokens = tokenize_tmux_line(line);
    if tokens.len() < 2 {
        return None;
    }

    if !matches!(tokens[0].as_str(), "set" | "set-option") {
        return None;
    }

    let index = tokens.iter().position(|token| token == "@plugin")?;
    tokens.get(index + 1).cloned()
}

fn parse_plugin_spec(raw: &str) -> Result<PluginSpec> {
    let (source, tracking) = match raw.rsplit_once('#') {
        Some((source, branch)) if !branch.is_empty() => {
            (source.to_string(), Tracking::Branch(branch.to_string()))
        }
        _ => (raw.to_string(), Tracking::DefaultBranch),
    };

    build_remote_plugin_spec(source, None, String::new(), tracking, None, Vec::new())
}

fn expand_source_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return home_dir();
    }
    if let Some(suffix) = raw.strip_prefix("~/") {
        return home_dir().join(suffix);
    }
    if raw == "$HOME" {
        return home_dir();
    }
    if let Some(suffix) = raw.strip_prefix("$HOME/") {
        return home_dir().join(suffix);
    }
    PathBuf::from(raw)
}

fn tokenize_tmux_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;

    for ch in line.chars() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '#' => break,
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}
