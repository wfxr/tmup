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
    command: Option<Commands>,
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
        None => run_tui().await,
        Some(Commands::Init) => run_init().await,
        Some(Commands::Install { id }) => run_install(id).await,
        Some(Commands::Update { id }) => run_update(id).await,
        Some(Commands::Restore { id }) => run_restore(id).await,
        Some(Commands::Clean) => run_clean(),
        Some(Commands::List) => run_list(),
        Some(Commands::Migrate) => {
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
        if path.exists() {
            return Ok(path);
        }
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

/// Acquire lock, replan, execute writes if needed.
async fn acquire_and_write(cfg: &lazytmux::model::Config, paths: &Paths) -> Result<()> {
    let _guard = OperationLock::try_acquire(&paths.lock_path)?
        .context("failed to acquire operation lock")?;
    let installed = planner::scan_installed_plugins(&paths.plugin_root);
    let mut lock = load_lockfile(paths)?;
    let decision = planner::plan_init(cfg, &lock, &installed, false);
    if let planner::InitDecision::Write(plan) = decision {
        run_init_write(cfg, &mut lock, paths, &plan).await?;
    }
    Ok(())
}

/// Writer-aware init flow (v5 design §5.2)
async fn run_init() -> Result<()> {
    let paths = Paths::resolve()?;
    paths.ensure_dirs()?;
    let cfg = load_config(&paths)?;
    let lock = load_lockfile(&paths)?;

    // Step 1: Read-only preflight
    let installed = planner::scan_installed_plugins(&paths.plugin_root);
    let writer_active = OperationLock::is_writer_active(&paths.lock_path)?;
    let decision = planner::plan_init(&cfg, &lock, &installed, writer_active);

    match decision {
        planner::InitDecision::ReadOnly => {
            // No writes needed, just load
        }
        planner::InitDecision::WaitForWriter => {
            // Wait for writer to finish, then acquire lock and replan
            eprintln!("lazytmux: waiting for active writer to finish...");
            wait_for_writer(&paths).await?;
            acquire_and_write(&cfg, &paths).await?;
        }
        planner::InitDecision::Write(_write_plan) => {
            // Try to acquire exclusive lock
            let guard = OperationLock::try_acquire(&paths.lock_path)?;
            if guard.is_none() {
                // Another writer beat us, wait then re-acquire
                eprintln!("lazytmux: lock contention, waiting...");
                wait_for_writer(&paths).await?;
                acquire_and_write(&cfg, &paths).await?;
            } else {
                // Replan inside lock
                let installed = planner::scan_installed_plugins(&paths.plugin_root);
                let mut lock = load_lockfile(&paths)?;
                let decision = planner::plan_init(&cfg, &lock, &installed, false);
                if let planner::InitDecision::Write(plan) = decision {
                    run_init_write(&cfg, &mut lock, &paths, &plan).await?;
                }
            }
        }
    }

    // Load plugins into tmux
    let plan = loader::build_load_plan(&cfg, &paths.plugin_root);
    tmux::execute_plan(&plan)?;

    // Optionally bind UI key
    if let Some(bind) = loader::build_bind_command(&cfg, "lazytmux") {
        let _ = tmux::execute(&bind);
    }

    Ok(())
}

async fn run_init_write(
    cfg: &lazytmux::model::Config,
    lock: &mut lockfile::LockFile,
    paths: &Paths,
    plan: &planner::WritePlan,
) -> Result<()> {
    // Install missing plugins, skipping known build failures
    for id in &plan.to_install {
        if let Some(spec) = cfg.plugins.iter().find(|p| p.remote_id() == Some(id))
            && let Some(build_cmd) = &spec.build
        {
            let is_failure = if let Some(entry) = lock.plugins.get(id.as_str()) {
                plugin::is_known_failure(paths, id, &entry.commit, build_cmd)?
            } else {
                // No lock entry (first-install failure): check by plugin + build command
                plugin::is_known_failure_for_build(paths, id, build_cmd)?
            };
            if is_failure {
                eprintln!("lazytmux: skipping {id} (known build failure)");
                continue;
            }
        }
        plugin::install(cfg, lock, paths, Some(id.as_str())).await?;
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
    plugin::install(&cfg, &mut lock, &paths, id.as_deref()).await
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

async fn wait_for_writer(paths: &Paths) -> Result<()> {
    for _ in 0..300 {
        if !OperationLock::is_writer_active(&paths.lock_path)? {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("timed out waiting for writer lock")
}

async fn run_tui() -> Result<()> {
    let paths = Paths::resolve()?;
    let cfg = load_config(&paths)?;
    let lock = load_lockfile(&paths)?;
    let statuses = plugin::list(&cfg, &lock, &paths)?;
    let busy = OperationLock::is_writer_active(&paths.lock_path)?;
    let app = lazytmux::ui::App::new(statuses, busy);
    lazytmux::ui::run_tui(app)
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
}
