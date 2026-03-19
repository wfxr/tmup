use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lazytmux::state::{OperationLock, Paths};
use lazytmux::sync::{self, SyncPolicy};
use lazytmux::{config, loader, lockfile, planner, plugin, tmux};

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
    /// Reconcile lock metadata and declared remote plugins with config
    Sync {
        /// Plugin id to sync (all if omitted)
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
        Commands::Sync { id } => run_sync(id).await,
        Commands::Update { id } => run_update(id).await,
        Commands::Restore { id } => run_restore(id).await,
        Commands::Clean => run_clean().await,
        Commands::List => run_list(),
        Commands::Migrate => {
            eprintln!("migrate not yet implemented");
            Ok(())
        }
    }
}

fn resolve_runtime_paths() -> Result<Paths> {
    let mut paths = Paths::resolve()?;
    let config_path = resolve_config_path(&paths)?;
    paths.set_config_path(config_path)?;
    Ok(paths)
}

fn load_config(paths: &Paths) -> Result<lazytmux::model::Config> {
    let content = std::fs::read_to_string(&paths.config_path)
        .with_context(|| format!("failed to read config: {}", paths.config_path.display()))?;
    config::parse_config(&content)
}

fn resolve_config_path(paths: &Paths) -> Result<std::path::PathBuf> {
    // 1. $LAZY_TMUX_CONFIG
    if let Ok(p) = std::env::var("LAZY_TMUX_CONFIG") {
        let path = std::path::PathBuf::from(p);
        anyhow::ensure!(path.exists(), "LAZY_TMUX_CONFIG={} does not exist", path.display());
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
    let paths = resolve_runtime_paths()?;
    paths.ensure_dirs()?;
    let cfg = load_config(&paths)?;

    // Hold the lock for the entire init: plan, mutate, and load.
    let _guard = OperationLock::acquire(&paths.lock_path)?;

    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, None, SyncPolicy::init(cfg.options.auto_install))
        .await?;
    let managed_ids = planner::scan_managed_plugin_ids(&paths.plugin_root);
    let health_map = build_health_map(&cfg, &paths);
    let plan = planner::plan_init(&cfg, &lock, &health_map, &managed_ids);

    let write_failures = if let Some(write_plan) = plan {
        run_init_write(&cfg, &mut lock, &paths, &write_plan).await
    } else {
        Vec::new()
    };

    // Always load plugins into tmux — partial write failures must not
    // prevent loading plugins that are already available on disk.
    let load_plan = loader::build_load_plan(&cfg, &paths.plugin_root);
    tmux::execute_plan(&load_plan)?;

    if !write_failures.is_empty() {
        anyhow::bail!(
            "init encountered {} failure(s):\n  {}",
            write_failures.len(),
            write_failures.join("\n  ")
        );
    }
    Ok(())
}

/// Run install/restore/clean writes. Returns a list of non-fatal failure
/// messages — the caller should still proceed with loading plugins.
async fn run_init_write(
    cfg: &lazytmux::model::Config,
    lock: &mut lockfile::LockFile,
    paths: &Paths,
    plan: &planner::WritePlan,
) -> Vec<String> {
    let mut failures: Vec<String> = Vec::new();

    // Install missing plugins (known build failures are suppressed inside install)
    for id in &plan.to_install {
        if let Err(e) = plugin::install(cfg, lock, paths, Some(id.as_str()), true).await {
            failures.push(format!("{e}"));
        }
    }

    // Restore plugins whose installed commit has drifted from the lock
    for id in &plan.to_restore {
        if let Err(e) = plugin::restore(cfg, lock, paths, Some(id.as_str())).await {
            failures.push(format!("{e}"));
        }
    }

    // Clean undeclared
    if !plan.to_clean.is_empty()
        && let Err(e) = plugin::clean(cfg, paths)
    {
        failures.push(format!("{e}"));
    }

    failures
}

async fn run_install(id: Option<String>) -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, id.as_deref(), SyncPolicy::INSTALL).await?;
    plugin::install(&cfg, &mut lock, &paths, id.as_deref(), false).await
}

async fn run_sync(id: Option<String>) -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, id.as_deref(), SyncPolicy::SYNC).await
}

async fn run_update(id: Option<String>) -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, id.as_deref(), SyncPolicy::UPDATE).await?;
    plugin::update(&cfg, &mut lock, &paths, id.as_deref()).await
}

async fn run_restore(id: Option<String>) -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, id.as_deref(), SyncPolicy::RESTORE).await?;
    plugin::restore(&cfg, &lock, &paths, id.as_deref()).await
}

async fn run_clean() -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("another lazytmux operation is in progress")?;
    let cfg = load_config(&paths)?;
    let mut lock = load_lockfile(&paths)?;
    sync::run_and_write(&cfg, &mut lock, &paths, None, SyncPolicy::CLEAN).await?;
    plugin::clean(&cfg, &paths)
}

fn run_list() -> Result<()> {
    let paths = resolve_runtime_paths()?;
    let cfg = load_config(&paths)?;
    let lock = load_lockfile(&paths)?;
    let statuses = plugin::list(&cfg, &lock, &paths)?;

    if sync::lock_is_stale(&cfg, &lock) {
        println!("warning: lock metadata is stale relative to config; run `lazytmux sync`");
    }

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
            short_commit(s.current_commit.as_deref()),
            short_commit(s.lock_commit.as_deref()),
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

fn short_commit(hash: Option<&str>) -> &str {
    hash.map(|c| &c[..7.min(c.len())]).unwrap_or("-")
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
}
