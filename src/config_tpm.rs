use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use glob::glob;
use regex::Regex;

use crate::model::{Config, Options, PluginSpec};
use crate::state::resolve_home_dir;

static TPM_PLUGIN_DECLARATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*set(?:-option)? +-g +@plugin(?: +|$)").expect("valid TPM plugin regex")
});

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourcedFile {
    path: String,
    quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of discovering the TPM-compatible tmux config path.
pub struct ResolvedConfigPath {
    /// The discovered TPM-compatible tmux config path, if one was found.
    pub path: Option<PathBuf>,
    /// Non-fatal warnings emitted while attempting to discover the config path.
    pub warnings: Vec<String>,
}

/// Resolve the default TPM-style tmux config path from the supported search order.
pub fn resolve_config_path() -> Result<ResolvedConfigPath> {
    let xdg_config_home = std::env::var("XDG_CONFIG_HOME").ok();
    let home_dir = match resolve_home_dir() {
        Ok(home_dir) => home_dir,
        Err(_) => {
            return Ok(ResolvedConfigPath {
                path: xdg_tmux_config_path(xdg_config_home.as_deref()),
                warnings: vec!["HOME is unavailable; skipping default TPM config discovery".into()],
            });
        }
    };
    Ok(ResolvedConfigPath {
        path: resolve_config_path_from_env(xdg_config_home.as_deref(), &home_dir),
        warnings: Vec::new(),
    })
}

fn xdg_tmux_config_path(xdg_config_home: Option<&str>) -> Option<PathBuf> {
    if let Some(xdg_config_home) = xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        let path = xdg_config_home.join("tmux/tmux.conf");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

fn resolve_config_path_from_env(xdg_config_home: Option<&str>, home_dir: &Path) -> Option<PathBuf> {
    if let Some(path) = xdg_tmux_config_path(xdg_config_home) {
        return Some(path);
    }

    let config_home = home_dir.join(".config/tmux/tmux.conf");
    if config_home.exists() {
        return Some(config_home);
    }

    let legacy_home = home_dir.join(".tmux.conf");
    if legacy_home.exists() {
        return Some(legacy_home);
    }

    None
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
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    // Intentionally matches current TPM behavior instead of inlining sourced
    // fragments at the directive position. TPM scans the main config first and
    // then appends direct sourced files afterward, so mixed-mode ordering here
    // must preserve that quirk for compatibility.
    //
    // TPM reference:
    // - `_tmux_conf_contents "full"` cats the main config before any sourced files
    // - `tpm_plugins_list_helper` parses plugin declarations from that combined stream
    //
    // Intentionally only scans direct sourced files discovered from the root
    // tmux config, matching TPM instead of recursively expanding nested includes.
    for sourced in direct_sourced_files(&root) {
        for sourced_path in expand_source_paths(&sourced.path, base_dir)? {
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
    token
        .strip_prefix('-')
        .is_some_and(|flags| !flags.is_empty() && flags.chars().all(|ch| ch == 'q'))
}

fn plugin_declaration(line: &str) -> Option<String> {
    // Intentionally mirrors TPM's current parser: only `set -g @plugin ...`
    // and `set-option -g @plugin ...` forms are treated as plugin declarations.
    if !TPM_PLUGIN_DECLARATION_RE.is_match(line) {
        return None;
    }

    let tokens = tokenize_tmux_line(line);
    if tokens.len() < 4 {
        return None;
    }

    if !matches!(tokens[0].as_str(), "set" | "set-option")
        || tokens.get(1).map(String::as_str) != Some("-g")
        || tokens.get(2).map(String::as_str) != Some("@plugin")
    {
        return None;
    }

    tokens.get(3).cloned()
}

fn parse_plugin_spec(raw: &str) -> Result<PluginSpec> {
    PluginSpec::from_tpm_remote(raw)
}

fn expand_source_paths(raw: &str, base_dir: &Path) -> Result<Vec<PathBuf>> {
    let expanded = shellexpand::full(raw)
        .with_context(|| format!("failed to expand sourced tmux config path: {raw}"))?
        .into_owned();
    let expanded_path = PathBuf::from(&expanded);
    let resolved =
        if expanded_path.is_absolute() { expanded_path } else { base_dir.join(expanded_path) };
    let resolved_display = resolved.to_string_lossy().into_owned();
    if !has_glob_pattern(&resolved_display) {
        return Ok(vec![resolved]);
    }
    let mut matches = Vec::new();
    for path in glob(&resolved_display)
        .with_context(|| format!("invalid sourced tmux config glob: {resolved_display}"))?
    {
        matches.push(path.with_context(|| {
            format!("failed to expand sourced tmux config glob: {resolved_display}")
        })?);
    }

    if matches.is_empty() { Ok(vec![resolved]) } else { Ok(matches) }
}

fn has_glob_pattern(value: &str) -> bool {
    value.chars().any(|ch| matches!(ch, '*' | '?' | '['))
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
            None if ch == '#' => {
                if current.is_empty() {
                    break;
                }
                current.push(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }

    // Keep TPM-style leniency here: unterminated quotes are treated as part of
    // the final token instead of rejecting the line with a parse error.
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    #[test]
    fn resolve_config_path_from_env_returns_none_when_missing() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        assert_eq!(super::resolve_config_path_from_env(None, &home), None);
    }
}
