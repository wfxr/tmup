pub mod config;
pub mod git;
pub mod loader;
pub mod lockfile;
pub mod model;
pub mod planner;
pub mod plugin;
pub mod state;
pub mod sync;
pub mod tmux;

/// Return the first 7 characters of a commit hash for display.
pub fn short_hash(hash: &str) -> &str {
    &hash[..7.min(hash.len())]
}
