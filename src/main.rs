use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use lazytmux::{
    config,
    loader,
    lockfile,
    planner,
    plugin,
    state::{OperationLock, Paths},
    tmux,
};

#[derive(Debug, Parser)]
#[command(name = "lazytmux", about = "Modern tmux plugin manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// tmux startup: install missing plugins, apply options, load plugins
    Init,
    /// Install missing remote plugins
    Install {
        /// Plugin id to install (all if omitted)
        id: Option<String>,
    },
    /// Update remote plugins (the only command that advances lock)
    Update {
        /// Plugin id to update (all if omitted)
        id: Option<String>,
    },
    /// Restore plugins to lock-recorded commits
    Restore {
        /// Plugin id to restore (all if omitted)
        id: Option<String>,
    },
    /// Remove undeclared managed remote plugins
    Clean,
    /// List plugin status
    List,
    /// Migrate from TPM .tmux.conf declarations
    Migrate,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => run_init().await,
        Commands::Install { id } => run_install(id).await,
        Commands::Update { id } => run_update(id).await,
        Commands::Restore { id } => run_restore(id).await,
        Commands::Clean => run_clean(),
        Commands::List => run_list(),
        Commands::Migrate => {
            eprintln!("migrate not yet implemented");
            Ok(())
        }
    }
}

fn load_config(paths: &Paths) -> Result<lazytmux::model::Config> {
    // Try config paths in order
    let config_path = resolve_config_path(paths)?;
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;
    config::parse_config(&content)
}

fn resolve_config_path(paths: &Paths) -> Result<std::path::PathBuf> {
    // 1. $LAZY_TMUX_CONFIG
    if let Ok(p) = std::env::var("LAZY_TMUX_CONFIG") {
        let path = std::path::PathBuf::from(p);
        anyhow::ensure!(
            path.exists(),
            "LAZY_TMUX_CONFIG={} does not exist",
            path.display()
        );
        return Ok(path);
    }
    // 2. Default config path
    if paths.config_path.exists() {
        return Ok(paths.config_path.clone());
    }
    // 3. ~/.tmux/lazy.kdl
    let home_tmux = dirs_home().join(".tmux/lazy.kdl");
    if home_tmux.exists() {
        return Ok(home_tmux);
    }
    anyhow::bail!(
        "config file not found. Create {} or set LAZY_TMUX_CONFIG",
        paths.config_path.display()
    )
}

fn load_lockfile(paths: &Paths) -> Result<lockfile::LockFile> {
    if paths.lockfile_path.exists() {
        lockfile::read_lockfile(&paths.lockfile_path)
    } else {
        Ok(lockfile::LockFile::new())
    }
}

/// Init flow: acquire the global lock, plan, mutate if needed, then load.
/// The lock is held from start to finish so no concurrent writer can modify
/// plugin state during init.
async fn run_init() -> Result<()> {
    let paths = Paths::resolve()?;
    paths.ensure_dirs()?;
    let cfg = load_config(&paths)?;

    // Hold the lock for the entire init: plan, mutate, and load.
    let _guard = OperationLock::acquire(&paths.lock_path)?;

    let managed_ids = planner::scan_managed_plugin_ids(&paths.plugin_root);
    let health_map = build_health_map(&cfg, &paths);
    let mut lock = load_lockfile(&paths)?;
    let plan = planner::plan_init(&cfg, &lock, &health_map, &managed_ids);

    if let Some(write_plan) = plan {
        run_init_write(&cfg, &mut lock, &paths, &write_plan).await?;
    }

    // Load plugins into tmux (still under lock)
    let load_plan = loader::build_load_plan(&cfg, &paths.plugin_root);
    tmux::execute_plan(&load_plan)?;

    Ok(())
}

async fn run_init_write(
    cfg: &lazytmux::model::Config,
    lock: &mut lockfile::LockFile,
    paths: &Paths,
    plan: &planner::WritePlan,
) -> Result<()> {
    // Install missing plugins (known build failures are suppressed inside install)
    for id in &plan.to_install {
        plugin::install(cfg, lock, paths, Some(id.as_str()), true).await?;
    }

    // Restore plugins whose installed commit has drifted from the lock
    for id in &plan.to_restore {
        plugin::restore(cfg, lock, paths, Some(id.as_str())).await?;
    }

    // Clean undeclared
    if !plan.to_clean.is_empty() {
        plugin::clean(cfg, paths)?;
    }

    Ok(())
}

async fn run_install(id: Option<String>) -> Result<()> {
    let paths = Paths::resolve()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    plugin::install(&cfg, &mut lock, &paths, id.as_deref(), false).await
}

async fn run_update(id: Option<String>) -> Result<()> {
    let paths = Paths::resolve()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    plugin::update(&cfg, &mut lock, &paths, id.as_deref()).await
}

async fn run_restore(id: Option<String>) -> Result<()> {
    let paths = Paths::resolve()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let lock = load_lockfile(&paths)?;
    plugin::restore(&cfg, &lock, &paths, id.as_deref()).await
}

fn run_clean() -> Result<()> {
    let paths = Paths::resolve()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    plugin::clean(&cfg, &paths)
}

fn run_list() -> Result<()> {
    let paths = Paths::resolve()?;
    let cfg = load_config(&paths)?;
    let lock = load_lockfile(&paths)?;
    let statuses = plugin::list(&cfg, &lock, &paths)?;

    // Print header
    println!(
        "{:<45} {:<20} {:<8} {:<15} {:<15} {:<12} {:<12} source",
        "id", "name", "kind", "state", "last-result", "current", "lock"
    );
    for s in &statuses {
        println!(
            "{:<45} {:<20} {:<8} {:<15} {:<15} {:<12} {:<12} {}",
            s.id,
            s.name,
            s.kind,
            s.state,
            s.last_result,
            s.current_commit
                .as_deref()
                .map(|c| &c[..7.min(c.len())])
                .unwrap_or("-"),
            s.lock_commit
                .as_deref()
                .map(|c| &c[..7.min(c.len())])
                .unwrap_or("-"),
            s.source,
        );
    }

    Ok(())
}

fn build_health_map(
    cfg: &lazytmux::model::Config,
    paths: &Paths,
) -> std::collections::HashMap<String, lazytmux::planner::RepoHealth> {
    cfg.plugins
        .iter()
        .filter_map(|spec| {
            let id = spec.remote_id()?;
            let health = lazytmux::planner::inspect_plugin_dir(&paths.plugin_dir(id));
            Some((id.to_string(), health))
        })
        .collect()
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
}
