use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;

use crate::model::Config;
use crate::state::Paths;
use crate::{config, config_tpm};

/// Supported configuration loading modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ConfigMode {
    /// Load only tmup.kdl.
    Tmup,
    /// Load both config sources and merge them.
    Mixed,
}

impl fmt::Display for ConfigMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tmup => f.write_str("tmup"),
            Self::Mixed => f.write_str("mixed"),
        }
    }
}

/// Loaded configuration plus non-fatal warnings encountered during load.
#[derive(Debug)]
pub struct LoadedConfig {
    /// The normalized effective configuration.
    pub config: Config,
    /// Warnings emitted while loading or merging config sources.
    pub warnings: Vec<String>,
    /// The resolved primary config path that should own the active lockfile.
    pub active_config_path: PathBuf,
    /// The resolved TPM config path when mixed mode discovered one.
    pub tpm_config_path: Option<PathBuf>,
}

/// Load configuration for the requested mode without creating a missing default tmup.kdl.
pub fn load(paths: &Paths, mode: ConfigMode) -> Result<LoadedConfig> {
    load_with_policy(paths, mode, false)
}

/// Load configuration for the requested mode, creating the default tmup.kdl when needed.
pub fn load_or_create_default(paths: &Paths, mode: ConfigMode) -> Result<LoadedConfig> {
    load_with_policy(paths, mode, true)
}

/// Ensure the active tmup.kdl exists on disk using the default template.
pub fn ensure_tmup_config_exists(paths: &Paths) -> Result<()> {
    let path = prepare_tmup_config_path(paths, false)?;
    if !path.exists() {
        create_default_tmup_config(&path)?;
    }
    Ok(())
}

/// Load configuration for the requested mode from explicit source paths.
pub fn load_from_sources(
    mode: ConfigMode,
    tmup_path: Option<&Path>,
    tpm_path: Option<&Path>,
) -> Result<LoadedConfig> {
    match mode {
        ConfigMode::Tmup => {
            let path = tmup_path.context("tmup config file not found")?;
            Ok(LoadedConfig {
                config: load_tmup_config(path)?,
                warnings: Vec::new(),
                active_config_path: path.to_path_buf(),
                tpm_config_path: None,
            })
        }
        ConfigMode::Mixed => load_mixed(tmup_path, tpm_path),
    }
}

fn load_mixed(tmup_path: Option<&Path>, tpm_path: Option<&Path>) -> Result<LoadedConfig> {
    let tmup_path = tmup_path.context("tmup config file not found")?;
    let mut warnings = Vec::new();
    let tpm_config = tpm_path.map(config_tpm::load_config_from_path).transpose()?;
    let tmup_config = load_tmup_config_or_default(tmup_path)?;

    match tpm_config {
        Some(tpm) => {
            let config = merge_configs(tmup_config, tpm, &mut warnings);
            Ok(LoadedConfig {
                config,
                warnings,
                active_config_path: tmup_path.to_path_buf(),
                tpm_config_path: tpm_path.map(Path::to_path_buf),
            })
        }
        None => Ok(LoadedConfig {
            config: tmup_config,
            warnings,
            active_config_path: tmup_path.to_path_buf(),
            tpm_config_path: None,
        }),
    }
}

fn load_with_policy(paths: &Paths, mode: ConfigMode, create_missing: bool) -> Result<LoadedConfig> {
    let tmup_path = prepare_tmup_config_path(paths, create_missing)?;
    match mode {
        ConfigMode::Tmup => Ok(LoadedConfig {
            config: if create_missing || tmup_path.exists() {
                load_tmup_config(&tmup_path)?
            } else {
                default_tmup_config()?
            },
            warnings: Vec::new(),
            active_config_path: tmup_path,
            tpm_config_path: None,
        }),
        ConfigMode::Mixed => {
            let resolved = config_tpm::resolve_config_path()?;
            let mut loaded =
                load_from_sources(mode, Some(tmup_path.as_path()), resolved.path.as_deref())?;
            loaded.warnings.extend(resolved.warnings);
            loaded.tpm_config_path = resolved.path;
            Ok(loaded)
        }
    }
}

fn load_tmup_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read tmup config: {}", path.display()))?;
    config::parse_config(&content)
}

fn load_tmup_config_or_default(path: &Path) -> Result<Config> {
    if path.exists() { load_tmup_config(path) } else { default_tmup_config() }
}

fn merge_configs(mut kdl: Config, tpm: Config, warnings: &mut Vec<String>) -> Config {
    let mut merged = Vec::with_capacity(tpm.plugins.len() + kdl.plugins.len());
    let mut kdl_remote_indices = std::collections::HashMap::new();

    for (index, plugin) in kdl.plugins.iter().enumerate() {
        if let Some(id) = plugin.remote_id() {
            kdl_remote_indices.insert(id.to_string(), index);
        }
    }

    let mut consumed_kdl = vec![false; kdl.plugins.len()];
    for plugin in tpm.plugins {
        let Some(id) = plugin.remote_id() else {
            continue;
        };

        if let Some(&index) = kdl_remote_indices.get(id) {
            merged.push(kdl.plugins[index].clone());
            consumed_kdl[index] = true;
            warnings.push(format!(
                "plugin \"{id}\" declared in both tmup.kdl and TPM config; using tmup.kdl entry"
            ));
        } else {
            merged.push(plugin);
        }
    }

    for (index, plugin) in kdl.plugins.drain(..).enumerate() {
        if !consumed_kdl[index] {
            merged.push(plugin);
        }
    }

    Config { options: kdl.options, plugins: merged }
}

fn prepare_tmup_config_path(paths: &Paths, create_missing: bool) -> Result<PathBuf> {
    let path = paths.config_path.clone();
    if create_missing && !path.exists() {
        create_default_tmup_config(&path)?;
    }
    Ok(path)
}

fn create_default_tmup_config(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("config path has no parent directory: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to create config: {}", path.display()));
        }
    };
    use std::io::Write;
    file.write_all(default_tmup_config_template().as_bytes())
        .with_context(|| format!("failed to write config: {}", path.display()))?;
    file.flush().with_context(|| format!("failed to flush config: {}", path.display()))?;
    Ok(())
}

fn default_tmup_config_template() -> &'static str {
    r#"// tmup configuration
// Add plugins here, for example:
// plugin "tmux-plugins/tmux-sensible"
//
// If you are migrating from TPM, you can temporarily use:
// tmup <command> --config-mode=mixed

options {
    auto-install #true
    concurrency 16
}
"#
}

fn default_tmup_config() -> Result<Config> {
    config::parse_config(default_tmup_config_template())
        .context("internal default tmup config invalid")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    #[test]
    fn create_default_tmup_config_does_not_overwrite_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tmup.kdl");
        std::fs::write(&path, "plugin \"user/custom\"\n").unwrap();

        super::create_default_tmup_config(&path).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "plugin \"user/custom\"\n");
    }
}
