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

impl ConfigMode {
    /// Return the CLI-facing lowercase spelling for this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tmup => "tmup",
            Self::Mixed => "mixed",
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
    pub active_config_path: Option<PathBuf>,
}

/// Load configuration for the requested mode using discovered paths.
pub fn load(paths: &Paths, mode: ConfigMode) -> Result<LoadedConfig> {
    let tmup_path = ensure_tmup_config_path(paths)?;
    let tpm_path = match mode {
        ConfigMode::Tmup => None,
        ConfigMode::Mixed => discover_tpm_config_path()?,
    };
    load_from_sources(mode, Some(tmup_path.as_path()), tpm_path.as_deref())
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
                active_config_path: Some(path.to_path_buf()),
            })
        }
        ConfigMode::Mixed => load_mixed(tmup_path, tpm_path),
    }
}

fn load_mixed(tmup_path: Option<&Path>, tpm_path: Option<&Path>) -> Result<LoadedConfig> {
    let tmup_path = tmup_path.context("tmup config file not found")?;
    let mut warnings = Vec::new();
    let tpm_config = tpm_path.map(config_tpm::load_config_from_path).transpose()?;
    let tmup_config = load_tmup_config(tmup_path)?;

    match tpm_config {
        Some(tpm) => {
            let config = merge_configs(tmup_config, tpm, &mut warnings);
            Ok(LoadedConfig { config, warnings, active_config_path: Some(tmup_path.to_path_buf()) })
        }
        None => Ok(LoadedConfig {
            config: tmup_config,
            warnings,
            active_config_path: Some(tmup_path.to_path_buf()),
        }),
    }
}

fn load_tmup_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)?;
    config::parse_config(&content)
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

fn ensure_tmup_config_path(paths: &Paths) -> Result<PathBuf> {
    let path = if let Ok(p) = std::env::var("TMUP_CONFIG") {
        PathBuf::from(p)
    } else {
        paths.config_path.clone()
    };

    if !path.exists() {
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
    std::fs::write(path, default_tmup_config_template())
        .with_context(|| format!("failed to create config: {}", path.display()))?;
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

fn discover_tpm_config_path() -> Result<Option<PathBuf>> {
    match config_tpm::resolve_config_path() {
        Ok(path) => Ok(Some(path)),
        Err(err) if err.to_string().contains("tmux config file not found") => Ok(None),
        Err(err) => Err(err),
    }
}
