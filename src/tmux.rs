use anyhow::{Result, bail};
use std::{path::PathBuf, process::Stdio};

/// Represents a tmux command to be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxCommand {
    SetEnvironment { key: String, value: String },
    SetOption { key: String, value: String },
    RunShell { script: PathBuf },
}

impl TmuxCommand {
    /// Convert to tmux CLI arguments.
    pub fn to_args(&self) -> Vec<String> {
        match self {
            Self::SetEnvironment { key, value } => {
                vec![
                    "set-environment".into(),
                    "-g".into(),
                    key.clone(),
                    value.clone(),
                ]
            }
            Self::SetOption { key, value } => {
                vec!["set".into(), "-g".into(), format!("@{key}"), value.clone()]
            }
            Self::RunShell { script } => {
                vec!["run-shell".into(), script.to_string_lossy().to_string()]
            }
        }
    }
}

/// Execute a single tmux command.
pub fn execute(cmd: &TmuxCommand) -> Result<()> {
    let args = cmd.to_args();
    let output = std::process::Command::new("tmux")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "tmux {} failed: {stderr}",
            args.first().unwrap_or(&String::new())
        );
    }
    Ok(())
}

/// Execute a sequence of tmux commands.
pub fn execute_plan(plan: &[TmuxCommand]) -> Result<()> {
    for cmd in plan {
        execute(cmd)?;
    }
    Ok(())
}
