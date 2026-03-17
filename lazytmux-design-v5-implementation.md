# lazytmux v5 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the first working `lazytmux` implementation from `lazytmux-design-v5.md`: lock-first remote plugin management, safe publish/rollback, tmux loader/init flow, machine-readable status output, and a minimal TUI shell.

**Architecture:** Create a new Rust binary crate at `lazytmux/` with a small set of focused modules: `config.rs` for KDL parsing and validation, `planner.rs` for read/write planning and status derivation, `state.rs` for paths/locks/failure markers, `git.rs` for fetch/checkout/publish, `loader.rs` + `tmux.rs` for tmux command orchestration, and `ui/` for the ratatui shell. Keep the command path lock-first and writer-aware: `init` does a read-only preflight first, waits if a writer is active, and only upgrades to exclusive mutation when the plan says state must change.

**Tech Stack:** Rust, `clap`, `tokio`, `kdl`, `serde_json`, `etcetera`, `fd-lock`, `tempfile`, `ratatui`, `crossterm`, `anyhow`, `thiserror`, `assert_cmd`, `predicates`.

---

### Task 1: Bootstrap the crate and CLI shell

**Files:**
- Create: `lazytmux/Cargo.toml`
- Create: `lazytmux/src/lib.rs`
- Create: `lazytmux/src/main.rs`
- Test: `lazytmux/tests/cli_help.rs`

**Step 1: Write the failing CLI smoke test**

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_lists_core_commands() {
    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("restore"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("migrate"));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test cli_help -q
```

Expected: FAIL because the crate does not exist yet.

**Step 3: Create the crate and minimal CLI**

Create `lazytmux/Cargo.toml` with:

```toml
[package]
name = "lazytmux"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "fs"] }

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

Create `lazytmux/src/main.rs` with:

```rust
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "lazytmux")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init,
    Install { id: Option<String> },
    Update { id: Option<String> },
    Restore { id: Option<String> },
    Clean,
    List,
    Migrate,
}

fn main() {
    let _cli = Cli::parse();
}
```

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test cli_help -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/Cargo.toml lazytmux/src/lib.rs lazytmux/src/main.rs lazytmux/tests/cli_help.rs
git commit -m "feat: bootstrap lazytmux cli crate"
```

### Task 2: Define the core data model and KDL config parser

**Files:**
- Create: `lazytmux/src/model.rs`
- Create: `lazytmux/src/config.rs`
- Modify: `lazytmux/src/lib.rs`
- Test: `lazytmux/tests/config_parse.rs`

**Step 1: Write the failing config parser tests**

```rust
use lazytmux::config::parse_config;

#[test]
fn parses_remote_and_local_plugins() {
    let input = r#"
        options { auto-install true }
        plugin "tmux-plugins/tmux-sensible"
        plugin "~/dev/my-plugin" local=true name="my-plugin-dev"
    "#;

    let cfg = parse_config(input).unwrap();
    assert_eq!(cfg.plugins.len(), 2);
    assert!(cfg.plugins[0].is_remote());
    assert!(cfg.plugins[1].is_local());
}

#[test]
fn rejects_multiple_tracking_selectors() {
    let input = r#"plugin "tmux-plugins/tmux-yank" branch="main" tag="v1.0.0""#;
    assert!(parse_config(input).is_err());
}

#[test]
fn rejects_local_plugin_with_tracking_selector() {
    let input = r#"plugin "~/dev/my-plugin" local=true branch="main""#;
    assert!(parse_config(input).is_err());
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test config_parse -q
```

Expected: FAIL because parser/model modules do not exist.

**Step 3: Write the minimal parser and model**

Create `lazytmux/src/model.rs` with the core shapes:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub options: Options,
    pub plugins: Vec<PluginSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    pub concurrency: usize,
    pub auto_install: bool,
    pub auto_clean: bool,
    pub bind_ui: bool,
    pub ui_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    Remote { raw: String },
    Local { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracking {
    DefaultBranch,
    Branch(String),
    Tag(String),
    Commit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    pub source: PluginSource,
    pub name: Option<String>,
    pub opt_prefix: String,
    pub tracking: Tracking,
    pub build: Option<String>,
    pub opts: Vec<(String, String)>,
}
```

Create `lazytmux/src/config.rs` with `pub fn parse_config(input: &str) -> anyhow::Result<Config>`.

Validation rules to implement now:

- remote plugin: exactly one of default/branch/tag/commit
- local plugin: no branch/tag/commit
- remote plugin always locked by design, so no `lock` field in the public model

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test config_parse -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/lib.rs lazytmux/src/model.rs lazytmux/src/config.rs lazytmux/tests/config_parse.rs
git commit -m "feat: add config model and kdl parser"
```

### Task 3: Implement source normalization and remote plugin identity

**Files:**
- Modify: `lazytmux/src/model.rs`
- Modify: `lazytmux/src/config.rs`
- Test: `lazytmux/tests/source_normalization.rs`

**Step 1: Write the failing normalization tests**

```rust
use lazytmux::config::parse_config;

#[test]
fn normalizes_github_shorthand_to_full_id() {
    let cfg = parse_config(r#"plugin "tmux-plugins/tmux-sensible""#).unwrap();
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/tmux-plugins/tmux-sensible");
}

#[test]
fn normalizes_ssh_git_url_to_full_id() {
    let cfg = parse_config(r#"plugin "git@github.com:user/repo.git""#).unwrap();
    assert_eq!(cfg.plugins[0].remote_id().unwrap(), "github.com/user/repo");
}

#[test]
fn rejects_duplicate_remote_ids() {
    let input = r#"
        plugin "tmux-plugins/tmux-sensible"
        plugin "https://github.com/tmux-plugins/tmux-sensible.git"
    "#;
    assert!(parse_config(input).is_err());
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test source_normalization -q
```

Expected: FAIL because remote identity normalization is not implemented.

**Step 3: Add canonical remote identity**

Extend `PluginSource::Remote` to store:

```rust
Remote {
    raw: String,
    id: String,
    resolved_url: String,
}
```

Normalization rules:

- `user/repo` -> `github.com/user/repo`
- strip `.git`
- `git@host:owner/repo.git` -> `host/owner/repo`
- preserve custom hosts such as `git.example.com/team/plugin`

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test source_normalization -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/model.rs lazytmux/src/config.rs lazytmux/tests/source_normalization.rs
git commit -m "feat: normalize remote plugin identities"
```

### Task 4: Add the lockfile model and atomic write path

**Files:**
- Create: `lazytmux/src/lockfile.rs`
- Modify: `lazytmux/src/lib.rs`
- Test: `lazytmux/tests/lockfile.rs`

**Step 1: Write the failing lockfile tests**

```rust
use lazytmux::lockfile::{read_lockfile, write_lockfile_atomic, LockEntry, LockFile};
use tempfile::tempdir;

#[test]
fn round_trips_lockfile_json() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazylock.json");
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "github.com/tmux-plugins/tmux-sensible".into(),
        LockEntry::branch("tmux-plugins/tmux-sensible", "main", "abc123"),
    );

    write_lockfile_atomic(&path, &lock).unwrap();
    let reread = read_lockfile(&path).unwrap();
    assert_eq!(reread.plugins.len(), 1);
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test lockfile -q
```

Expected: FAIL because `lockfile.rs` does not exist.

**Step 3: Implement lockfile read/write**

Create `lazytmux/src/lockfile.rs` with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LockFile {
    pub version: u32,
    pub plugins: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockEntry {
    pub source: String,
    pub tracking: TrackingRecord,
    pub commit: String,
}
```

Atomic write contract:

- write to `lazylock.json.tmp`
- `sync_all`
- `rename` to target

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test lockfile -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/lib.rs lazytmux/src/lockfile.rs lazytmux/tests/lockfile.rs
git commit -m "feat: add lockfile roundtrip and atomic writes"
```

### Task 5: Add state paths, operation locking, and failure markers

**Files:**
- Create: `lazytmux/src/state.rs`
- Test: `lazytmux/tests/state.rs`

**Step 1: Write the failing state tests**

```rust
use lazytmux::state::{build_command_hash, FailureKey, Paths};

#[test]
fn paths_keep_plugins_and_staging_on_same_data_root() {
    let paths = Paths::for_test("/tmp/data", "/tmp/state");
    assert_eq!(paths.plugin_root.parent().unwrap(), paths.staging_root.parent().unwrap());
    assert_eq!(paths.plugin_root.parent().unwrap(), paths.backup_root.parent().unwrap());
}

#[test]
fn failure_key_changes_when_build_command_changes() {
    let a = FailureKey::new("github.com/user/repo", "abc123", &build_command_hash("make install"));
    let b = FailureKey::new("github.com/user/repo", "abc123", &build_command_hash("just build"));
    assert_ne!(a, b);
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test state -q
```

Expected: FAIL because state primitives do not exist.

**Step 3: Implement state primitives**

Create:

- `Paths { plugin_root, staging_root, backup_root, failures_root, lock_path }`
- `OperationLock`
- `FailureMarker`
- `FailureKey { plugin_id, commit, build_hash }`

Use filesystem layout:

```text
data_root/plugins/
data_root/.staging/
data_root/.backup/
state_root/operations.lock
state_root/failures/
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test state -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/state.rs lazytmux/tests/state.rs
git commit -m "feat: add state paths locks and failure markers"
```

### Task 6: Build the planner and status model

**Files:**
- Create: `lazytmux/src/planner.rs`
- Modify: `lazytmux/src/model.rs`
- Test: `lazytmux/tests/planner.rs`

**Step 1: Write the failing planner tests**

```rust
use lazytmux::planner::{LastResult, PluginState, Planner};

#[test]
fn read_only_init_plan_is_detected() {
    // config + installed + lock already aligned
    // expected: no writes required
}

#[test]
fn build_failure_keeps_state_and_result_separate() {
    assert_eq!(PluginState::Installed.to_string(), "installed");
    assert_eq!(LastResult::BuildFailed.to_string(), "build-failed");
}

#[test]
fn init_waits_when_writer_is_active() {
    // expected plan outcome: WaitForWriterThenReplan
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test planner -q
```

Expected: FAIL because planner types do not exist.

**Step 3: Implement planner core**

Create:

```rust
pub enum PluginState {
    Installed,
    Missing,
    Outdated,
    PinnedTag,
    PinnedCommit,
    Unmanaged,
}

pub enum LastResult {
    Ok,
    BuildFailed,
    None,
}

pub enum InitDecision {
    ReadOnly,
    WaitForWriter,
    Write(WritePlan),
}
```

Planner responsibilities:

- classify current remote/local plugin state
- separate `state` from `last_result`
- detect read-only vs write init path
- mark writer-aware wait before loading

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test planner -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/model.rs lazytmux/src/planner.rs lazytmux/tests/planner.rs
git commit -m "feat: add planner and split state result model"
```

### Task 7: Implement publish/rollback primitives

**Files:**
- Create: `lazytmux/src/git.rs`
- Modify: `lazytmux/src/state.rs`
- Test: `lazytmux/tests/publish.rs`

**Step 1: Write the failing publish tests**

```rust
use lazytmux::git::{publish_fresh_install, publish_replace};

#[test]
fn fresh_install_moves_staging_to_target() {
    // create staging dir, publish, assert target exists
}

#[test]
fn replace_rolls_back_when_build_fails() {
    // old target remains visible after simulated build error
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test publish -q
```

Expected: FAIL because publish helpers do not exist.

**Step 3: Implement publish helpers**

Required API shape:

```rust
pub fn publish_fresh_install(staging: &Path, target: &Path) -> anyhow::Result<()>;

pub fn publish_replace(
    staging: &Path,
    target: &Path,
    backup_root: &Path,
    build: impl FnOnce(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()>;
```

Rules:

- fresh install: `rename(staging, target)`, run build in final dir, delete target on build failure
- replace: `target -> backup`, `staging -> target`, run build, delete backup on success, rollback on failure

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test publish -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/git.rs lazytmux/src/state.rs lazytmux/tests/publish.rs
git commit -m "feat: add publish and rollback protocol"
```

### Task 8: Implement tmux command planning and loader order

**Files:**
- Create: `lazytmux/src/tmux.rs`
- Create: `lazytmux/src/loader.rs`
- Test: `lazytmux/tests/loader.rs`

**Step 1: Write the failing loader tests**

```rust
#[test]
fn loader_sets_env_then_opts_then_runs_tmux_files_in_order() {
    // expected tmux command plan:
    // 1) set-environment -g TMUX_PLUGIN_MANAGER_PATH ...
    // 2) set -g @...
    // 3) run-shell plugin_a/00-a.tmux
    // 4) run-shell plugin_a/10-b.tmux
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test loader -q
```

Expected: FAIL because loader modules do not exist.

**Step 3: Implement tmux command planning**

Do not shell out in tests. Build a command plan first:

```rust
pub enum TmuxCommand {
    SetEnvironment { key: String, value: String },
    SetOption { key: String, value: String },
    RunShell { script: PathBuf },
    BindPopup { key: String, command: String },
    BindSplit { key: String, command: String },
}
```

`loader.rs` should expose:

```rust
pub fn build_load_plan(...) -> Vec<TmuxCommand>;
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test loader -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/tmux.rs lazytmux/src/loader.rs lazytmux/tests/loader.rs
git commit -m "feat: add tmux command planning and loader ordering"
```

### Task 9: Implement install/update/restore/clean/list operations

**Files:**
- Create: `lazytmux/src/plugin.rs`
- Modify: `lazytmux/src/git.rs`
- Modify: `lazytmux/src/planner.rs`
- Test: `lazytmux/tests/operations.rs`

**Step 1: Write the failing operation tests**

```rust
#[tokio::test]
async fn install_uses_lock_entry_when_present() {
    // fake remote + lock entry -> install exact commit
}

#[tokio::test]
async fn update_advances_branch_and_rewrites_lock() {
    // fake branch repo -> update commit and lock
}

#[tokio::test]
async fn restore_rolls_back_and_runs_build() {
    // restore previous commit, build called in final target
}

#[tokio::test]
async fn clean_only_removes_managed_remote_plugins() {
    // local source untouched, unmanaged dirs untouched
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test operations -q
```

Expected: FAIL because the operation layer does not exist.

**Step 3: Implement minimal operation engine**

Expose async entry points:

```rust
pub async fn install(... ) -> anyhow::Result<()>;
pub async fn update(... ) -> anyhow::Result<()>;
pub async fn restore(... ) -> anyhow::Result<()>;
pub async fn clean(... ) -> anyhow::Result<()>;
pub async fn list(... ) -> anyhow::Result<Vec<ListRow>>;
```

Rules to enforce immediately:

- remote plugins always lock
- `tag` and `commit` are pinned in `update`
- restore runs build in final dir when revision changes
- list returns both `state` and `last_result`

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test operations -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/plugin.rs lazytmux/src/git.rs lazytmux/src/planner.rs lazytmux/tests/operations.rs
git commit -m "feat: add core plugin operations"
```

### Task 10: Implement the writer-aware `init` path

**Files:**
- Modify: `lazytmux/src/main.rs`
- Modify: `lazytmux/src/plugin.rs`
- Modify: `lazytmux/src/state.rs`
- Test: `lazytmux/tests/init_flow.rs`

**Step 1: Write the failing init flow tests**

```rust
#[tokio::test]
async fn init_read_only_path_does_not_take_writer_lock() {
    // already aligned config/lock/install state
}

#[tokio::test]
async fn init_replans_inside_lock_before_mutation() {
    // stale preflight result must be recomputed after lock acquisition
}

#[tokio::test]
async fn init_waits_for_writer_before_read_only_load() {
    // if a writer lock exists, init should wait and then redo preflight
}

#[tokio::test]
async fn init_does_not_retry_same_failed_build_tuple() {
    // failure marker with same id/commit/build hash suppresses auto retry
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test init_flow -q
```

Expected: FAIL because init orchestration is not implemented.

**Step 3: Implement `init` orchestration**

Add an async entry point like:

```rust
pub async fn run_init(ctx: &AppContext) -> anyhow::Result<()>;
```

Required behavior:

- preflight config + lock + install state + failure markers
- read-only path waits if writer active
- write path acquires exclusive lock, replans, mutates, then loads
- no retry for identical `(plugin id, commit, build hash)` failures

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test init_flow -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/main.rs lazytmux/src/plugin.rs lazytmux/src/state.rs lazytmux/tests/init_flow.rs
git commit -m "feat: add writer-aware init flow"
```

### Task 11: Wire `list` output and end-to-end CLI tests

**Files:**
- Modify: `lazytmux/src/main.rs`
- Test: `lazytmux/tests/cli_list.rs`

**Step 1: Write the failing CLI list tests**

```rust
use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn list_prints_state_and_last_result_columns() {
    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("list")
        .assert()
        .success()
        .stdout(contains("state"))
        .stdout(contains("last-result"));
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test cli_list -q
```

Expected: FAIL because `list` output is not wired.

**Step 3: Implement CLI rendering**

Render a stable machine-readable table or TSV with columns:

```text
id  name  kind  state  last-result  current-commit  lock-commit  source
```

Do not hide `last-result`.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test cli_list -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/main.rs lazytmux/tests/cli_list.rs
git commit -m "feat: add list output with state and result columns"
```

### Task 12: Add the TUI shell and busy/status presentation

**Files:**
- Create: `lazytmux/src/ui/mod.rs`
- Create: `lazytmux/src/ui/plugin_list.rs`
- Create: `lazytmux/src/ui/progress.rs`
- Create: `lazytmux/src/ui/detail.rs`
- Modify: `lazytmux/src/main.rs`
- Test: `lazytmux/tests/ui_smoke.rs`

**Step 1: Write the failing TUI smoke tests**

```rust
#[test]
fn ui_renders_state_and_last_result() {
    // render app state to buffer and assert both "installed" and "build-failed" are visible
}

#[test]
fn ui_shows_busy_banner_when_writer_is_active() {
    // render with busy flag true
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test ui_smoke -q
```

Expected: FAIL because UI modules do not exist.

**Step 3: Implement the minimal ratatui shell**

Required first-pass behavior:

- `lazytmux` with no subcommand enters TUI
- render rows using planner/list data
- show `state`
- show `last-result`
- show `busy` banner if writer active

Minimal app state:

```rust
pub struct App {
    pub rows: Vec<ListRow>,
    pub busy: bool,
    pub selected: usize,
}
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml --test ui_smoke -q
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux/src/ui/mod.rs lazytmux/src/ui/plugin_list.rs lazytmux/src/ui/progress.rs lazytmux/src/ui/detail.rs lazytmux/src/main.rs lazytmux/tests/ui_smoke.rs
git commit -m "feat: add initial tui shell"
```

### Task 13: Run the full verification suite and tighten rough edges

**Files:**
- Modify: `lazytmux/Cargo.toml`
- Modify: `lazytmux/src/*.rs`
- Modify: `lazytmux/tests/*.rs`

**Step 1: Run the full test suite**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml
```

Expected: PASS for all unit, integration, CLI, and TUI smoke tests.

**Step 2: Run formatting and linting**

Run:

```bash
cargo fmt --manifest-path lazytmux/Cargo.toml --check
cargo clippy --manifest-path lazytmux/Cargo.toml --all-targets -- -D warnings
```

Expected: PASS with no formatting drift and no clippy warnings.

**Step 3: Fix only the failing edge cases**

Target classes of fixes:

- unstable test fixtures
- path handling across local/remote plugins
- stale failure marker cleanup
- CLI/TUI state/result rendering mismatches

**Step 4: Re-run verification**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml
cargo fmt --manifest-path lazytmux/Cargo.toml --check
cargo clippy --manifest-path lazytmux/Cargo.toml --all-targets -- -D warnings
```

Expected: PASS.

**Step 5: Commit**

```bash
git add lazytmux
git commit -m "chore: stabilize lazytmux mvp implementation"
```

### Task 14: Manual tmux verification

**Files:**
- Modify: `lazytmux/tests/integration/README.md`

**Step 1: Write the manual verification checklist**

Add a small checklist file that covers:

- fresh init with lock present
- read-only init with no writes
- writer active + read-only init wait path
- build failure then explicit retry after changing `build`
- popup binding on tmux >= 3.2
- split fallback on older tmux

**Step 2: Run a real local smoke test**

Run:

```bash
tmux -L lazytmux-test -f /dev/null new-session -d "sleep 30"
TMUX= tmux -L lazytmux-test source-file /path/to/your/test.conf
```

Expected: `lazytmux init` completes, plugins load, and no partial plugin dir remains after simulated build failure.

**Step 3: Capture follow-up bugs as issues, not scope creep**

Only fix blockers to the v5 core semantics:

- lock-first behavior
- publish rollback
- writer-aware init
- state + last-result reporting

**Step 4: Re-run the affected automated tests**

Run:

```bash
cargo test --manifest-path lazytmux/Cargo.toml tests:: -- --nocapture
```

Expected: PASS for the affected suites.

**Step 5: Commit**

```bash
git add lazytmux/tests/integration/README.md
git commit -m "docs: add lazytmux manual verification checklist"
```
