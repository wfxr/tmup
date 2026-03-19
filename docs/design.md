# lazy.tmux Design

## Context

TPM (tmux plugin manager) has been unmaintained for years. It suffers from
structural limitations: pure bash implementation, weak error handling, serial
install/update, no lock file, and no reproducible state management.

lazy.tmux is a Rust-based tmux plugin manager inspired by lazy.nvim's design
philosophy: concise configuration, concurrent operations, reproducible
environments, and safe publish/rollback semantics.

The project focuses exclusively on CLI-driven workflows. The core value is
config-driven sync, safe publish and rollback, lock-through-load init, and
script-friendly behavior with reliable exit codes.

---

## 1. Design Goals and Non-Goals

### Goals

1. **Reproducible**: the same `lazy.kdl` + `lazylock.json` snapshot produces
   identical plugin versions on any machine.
2. **Compatible with common TPM plugins**: supports plugins that work through
   `*.tmux` entry scripts and `@option` settings.
3. **Fast startup**: `init` in the "all installed, lock unchanged" case is
   near-zero overhead.
4. **Safe under concurrency**: multiple tmux clients, shells, or concurrent
   CLI invocations cannot corrupt plugin directories or the lock file.
5. **Script and automation friendly**: clear exit codes, partial-failure
   reporting, and predictable CLI semantics.

### Non-Goals

1. **Not a TPM clone**: compatibility targets the common plugin interface, not
   TPM's internal behavior.
2. **No reuse of TPM install directories**: migrating from TPM involves fresh
   clones.
3. **No compatibility with plugins that depend on TPM's flat directory
   layout**: e.g. plugins that enumerate `$TMUX_PLUGIN_MANAGER_PATH` children.
4. **No implicit updates during init**: startup may install missing plugins
   but never advances existing plugin versions or retries a known-failed build
   tuple.
5. **No TUI in MVP**: the interactive terminal UI is deferred. CLI commands
   and `list` output cover all current use cases.
6. **No hooks, registry, or dependency resolution in MVP**: these belong to
   future extensions.
7. **No support for out-of-band filesystem manipulation inside the managed
   plugin root**: manually cloned repos, in-place repo edits, and symlink-based
   layouts under `plugin_root` are outside the current contract.

---

## 2. Core Principles

| Principle | Description |
|-----------|-------------|
| Config-driven sync | `lazy.kdl` is the desired state for remote plugins; `lazylock.json` is the resolved snapshot that mutating commands sync first. |
| Install concurrent, load serial | Git operations may run concurrently. Tmux option setting and plugin loading execute serially in declaration order. |
| URL-derived identity | Remote plugin IDs are derived from canonical source URLs, avoiding conflicts and manual naming. |
| Zero magic options | `opt-prefix` defaults to empty. No automatic prefix inference or separator insertion. |
| Compatibility by contract | Explicitly declare which TPM behaviors are supported and which are not. |
| Safe publish | New revisions are prepared in a staging directory, then atomically published with rollback capability. |
| Lock-through-load | `init` holds the global lock from entry through loading, so no concurrent writer can modify state mid-init. |
| Partial failure is an error | Commands that encounter per-plugin failures still publish successful results but return a non-zero exit code. |

---

## 3. Plugin Identity and Path Model

### 3.1 Remote Plugin Identity

All remote plugin IDs are derived from the source URL, similar to Go module
paths:

| Source form | Derived ID |
|---|---|
| `tmux-plugins/tmux-sensible` | `github.com/tmux-plugins/tmux-sensible` |
| `https://github.com/user/repo.git` | `github.com/user/repo` |
| `https://gitlab.com/user/plugin.git` | `gitlab.com/user/plugin` |
| `git@github.com:user/repo.git` | `github.com/user/repo` |
| `https://git.example.com/team/plugin.git` | `git.example.com/team/plugin` |

Rules:

- `id` is the **unique primary key** for remote plugins.
- Lock key = id.
- Install directory = `{plugin_root}/{id}/`.
- `name` defaults to the last segment of the ID (basename); used only for
  display in `list` output and log messages.
- The CLI target selector is `id`, not `name`.
- The managed plugin tree is lazy.tmux-owned state; manually cloning into it,
  mutating repos in place, or introducing symlink-based layouts is unsupported.

Directory layout:

```text
~/.local/share/lazytmux/plugins/
  +-- github.com/
  |   +-- tmux-plugins/
  |   |   +-- tmux-sensible/
  |   |   +-- tmux-resurrect/
  |   |   +-- tmux-yank/
  |   +-- catppuccin/
  |       +-- tmux/
  +-- gitlab.com/
      +-- user/
          +-- plugin/
```

### 3.2 Local Plugins

Local plugins are not cloned, not written to the lock snapshot, and do not
participate in `sync` / `install` / `update` / `restore`.

- Source must be a local path.
- `~`, `$VAR`, and `${VAR}` are expanded before validation.
- After expansion, the source must be an absolute path.
- Loaded in-place by lazy.tmux.
- `name` is for display only.
- `clean` never removes local plugin sources.

### 3.3 Why Full-Path Layout

1. **Naturally unique**: avoids basename collisions between `user1/tmux-x` and
   `user2/tmux-x`.
2. **No extra ID configuration**: remote plugins are stably addressable by
   default.
3. **Lock / install path / CLI selector are the same**: simpler documentation,
   state, and implementation.

Trade-off: `TMUX_PLUGIN_MANAGER_PATH` no longer has TPM's flat layout where
direct children are plugin directories. This is an explicit non-goal.

---

## 4. Configuration (KDL)

```kdl
// ~/.config/tmux/lazy.kdl

options {
    concurrency 8
    auto-install #true
    auto-clean #false
}

// Simplest form: GitHub shorthand, track default branch
plugin "tmux-plugins/tmux-sensible"

// Pin to a tag: update will skip this plugin
plugin "tmux-plugins/tmux-yank" tag="v2.3"

// Full config: branch + build + opts
plugin "tmux-plugins/tmux-resurrect" branch="master" build="make install" {
    opt "resurrect-strategy-vim" "session"
    opt "resurrect-save-bash-history" "on"
}

// opt-prefix reduces repetition
plugin "catppuccin/tmux" opt-prefix="catppuccin_" {
    opt "flavor" "mocha"          // -> @catppuccin_flavor "mocha"
    opt "window_text" "#W"        // -> @catppuccin_window_text "#W"
}

// Non-GitHub source
plugin "https://gitlab.com/user/my-plugin.git"

// Local plugin: loaded in-place, not managed by the lock snapshot
plugin "~/dev/my-tmux-plugin" local=#true name="my-plugin-dev"

// Disable a plugin with KDL slashdash
/-plugin "tmux-plugins/tmux-continuum"
```

### 4.1 Option Mechanism

Formula:

```text
set -g @{opt-prefix}{key} "{value}"
```

Rules:

- `opt-prefix` defaults to `""`.
- No automatic prefix inference.
- No automatic `-` or `_` separator.
- The user is responsible for the final tmux option name.

### 4.2 Plugin Properties

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| First argument | string | -- | GitHub `user/repo`, full git URL, or local path |
| `name` | string | remote: ID basename; local: path basename | Display name for list/logs |
| `opt-prefix` | string | `""` | Prefix prepended to opt keys |
| `branch` | string | default branch | Track a specific branch |
| `tag` | string | -- | Pinned release selector; `update` skips by default |
| `commit` | string | -- | Fixed commit; `update` skips |
| `local` | bool | `false` | Local path plugin, loaded in-place |
| `build` | string | -- | Executed in the final plugin directory after publish |
| `opt` | child node | -- | Becomes `set -g @{opt-prefix}{key} "{value}"` |

### 4.3 Global Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `concurrency` | int | `8` | Max parallel git operations (planned, currently serial) |
| `auto-install` | bool | `true` | Install missing plugins during `init` |
| `auto-clean` | bool | `false` | Remove undeclared plugins during `init` |

### 4.4 Validation Rules

1. Remote plugin IDs must be unique; duplicates cause an error.
2. `branch`, `tag`, `commit` are mutually exclusive.
3. `local=true` requires a path that expands to an absolute local path.
4. Remote plugins always enter the lock snapshot after successful sync; local plugins never do.
5. Local plugins cannot declare `branch` / `tag` / `commit`.

### 4.5 Lock-Affecting Inputs

Sync fingerprints only the remote plugin inputs that affect the resolved lock
snapshot:

- canonical remote plugin source
- tracking selector kind/value (`default-branch`, `branch`, `tag`, `commit`)
- `build`

The raw KDL text is not hashed. Comments, formatting, `name`, `opt`,
`opt-prefix`, and local-plugin-only changes must not trigger sync.

---

## 5. Command Semantics

### 5.1 Command Overview

```text
lazytmux init               # tmux startup: install missing, apply opts, load plugins
lazytmux sync [id]          # reconcile config into the lock snapshot
lazytmux install [id]       # install all/specified missing remote plugins after sync
lazytmux update [id]        # update unchanged floating selectors after sync
lazytmux restore [id]       # restore plugins to lock-recorded commits
lazytmux clean              # remove undeclared managed remote plugins
lazytmux list               # list plugin status
lazytmux migrate            # migrate from .tmux.conf TPM declarations (planned)
```

The CLI target selector is the **remote plugin ID**. `name` is for display
only. Local plugins do not participate in `sync` / `install` / `update` / `restore`.

### 5.2 `init` (tmux startup path)

Must be both fast and safe. The global operation lock is held for the entire
init (from scan through loading), eliminating TOCTOU races between preflight
and mutation.

```text
1. Acquire the global operation lock (blocking).
2. Parse lazy.kdl, read lazylock.json, validate configuration.
3. Run implicit incremental sync.
   - Reconcile changed existing declared plugins immediately.
   - Install newly declared remote plugins only when auto-install=true.
   - Drop removed remote plugins from the lock snapshot immediately.
   - Do not delete undeclared plugin directories here; that remains the job of
     `clean` / `auto-clean`.
   - Abort init if sync fails.
4. Scan installed remote plugin directories and their HEAD commits.
5. Compute whether additional lock-vs-disk writes are needed.
6. If writes are needed:
   - If auto-install=true: install missing remote plugins from the synced lock snapshot
   - If installed commit has drifted from lock: restore to the synced lock commit
   - If auto-clean=true: remove undeclared managed remote plugins from disk
7. Load plugins into tmux (set options, source *.tmux files).
8. Release the lock.
```

Key constraints:

- `init` **never advances** unchanged floating selectors beyond what config declares.
- `init` **does not retry** a known-failed `(plugin-id, commit, build-command-hash)` tuple.
- When all plugins are installed and lock is unchanged, init performs no git
  network access.

### 5.3 `sync [id]`

- Public command that reconciles remote plugin config into `lazylock.json`.
- Uses canonical remote plugin IDs as selectors.
- Applies source / selector / `build` changes incrementally by plugin ID.
- Removed remote plugins lose their lock entries immediately.
- Does not delete undeclared plugin directories from disk.
- On per-plugin failure, the previous lock entry for that plugin is preserved.

### 5.4 `install [id]`

- Runs implicit sync first and then installs missing remote plugins from the
  post-sync lock snapshot.
- Already-installed plugins are skipped only when they are already tracked in the lock snapshot.
- Explicit `install` does **not** suppress known-failure retries (unlike
  `init`).
- Returns a non-zero exit code if any plugin fails, after publishing
  successful ones and writing the lock snapshot.

### 5.5 `update [id]`

`update` runs after implicit sync. Selector or `build` changes are handled by
sync; `update` is responsible only for advancing unchanged floating selectors.

Revision policy:

- `branch`: fetch and update to the remote branch's latest commit.
- No `branch`/`tag`/`commit` specified: track the default branch's latest
  commit.
- `tag`: treated as a pinned release selector; skipped with a message.
- `commit`: fixed version; skipped with a message.

Returns a non-zero exit code if any plugin fails, after publishing successful
ones and writing the lock snapshot.

### 5.6 `restore [id]`

- Runs implicit sync first and then checks out remote plugins to the commit
  recorded in the post-sync lock snapshot.
- Requires a lock entry; plugins without one are skipped.
- Missing plugins are re-installed from the lock.
- If the revision actually changes and the plugin declares a `build`, the
  build runs after restore.
- Build failures during restore write failure markers, matching the semantics
  of install and update.

### 5.7 `clean`

- Runs a prune-only implicit sync first.
- Removes installed but undeclared remote plugins from the managed directory.
- Only cleans lazy.tmux-managed remote directories.
- Does not remove local plugin sources.
- Must not install, rebuild, replace, or otherwise mutate declared plugin
  directories as a side effect.
- Cleans up empty intermediate parent directories after removal.

### 5.8 `list`

Columns:

| Column | Description |
|--------|-------------|
| `id` | Canonical remote plugin ID or local path |
| `name` | Display name |
| `kind` | `remote` / `local` |
| `state` | `installed` / `missing` / `outdated` / `broken` / `pinned-tag` / `pinned-commit` / `local` |
| `last-result` | `ok` / `build-failed` / `none` |
| `current` | Installed HEAD commit (short hash or `-`) |
| `lock` | Lock-recorded commit (short hash or `-`) |
| `source` | Original source string |

State semantics:

- `outdated`: plugin directory exists but its HEAD commit differs from the
  lock-recorded commit.
- `missing`: plugin directory does not exist.

Last-result semantics:

- Determined by the presence of uncleared failure markers for the plugin's
  `(plugin-id, build-hash)` pair.
- Successful install/update/restore clears all failure markers for that plugin.
- Any uncleared marker means the last build operation failed, regardless of
  whether the marker's commit matches the current lock entry.

Examples:

- Update build failure with rollback: `state=installed`, `last-result=build-failed`
- Fresh install build failure: `state=missing`, `last-result=build-failed`

`list` is read-only. If the lock snapshot is stale relative to `lazy.kdl`,
it prints a warning before the table and does not mutate `lazylock.json`.

### 5.9 `migrate` (planned)

- Extracts `set -g @plugin` and related `set -g @xxx` options from
  `.tmux.conf`.
- When `opt-prefix` cannot be reliably inferred, generates a TODO comment
  instead of guessing.
- Does not overwrite an existing `lazy.kdl`.

---

## 6. Lock File

### 6.1 Format

```json
{
  "version": 2,
  "config_fingerprint": "b4a0d7c2...",
  "plugins": {
    "github.com/tmux-plugins/tmux-sensible": {
      "source": "tmux-plugins/tmux-sensible",
      "tracking": { "type": "default-branch", "value": "main" },
      "commit": "abc1234567890abcdef1234567890abcdef1234",
      "config_hash": "c78128e1..."
    },
    "github.com/tmux-plugins/tmux-resurrect": {
      "source": "tmux-plugins/tmux-resurrect",
      "tracking": { "type": "branch", "value": "master" },
      "commit": "def5678901234567890abcdef1234567890abcd",
      "config_hash": "89ce7bd4..."
    }
  }
}
```

### 6.2 Semantics

- Lock key = remote plugin ID.
- `config_fingerprint` hashes the sorted remote plugin desired-state inputs.
- Each `config_hash` hashes one remote plugin's lock-affecting config input.
- `tracking.type = "default-branch"` preserves declared selector semantics,
  while `tracking.value` stores the resolved branch name.
- `sync` resolves config into lock entries; `update` only advances unchanged
  floating selectors after sync.
- `restore`: strictly targets the lock-recorded commit.
- Local plugins are never in the lock snapshot. Remote plugins always are
  after first successful sync/install.
- Removed remote plugins lose their lock entries immediately, but `sync` does
  not delete their on-disk repositories.
- On partial failure: successfully published plugins update the lock; failed
  ones retain their previous entries.

### 6.3 Write Strategy

1. Serialize to `lazylock.json.tmp`.
2. `fsync`.
3. `rename` to `lazylock.json`.
4. Explicit sync and implicit sync preflights may still write updated metadata
   even when some plugins fail, preserving previous entries for failed plugins.

### 6.4 Read Error Handling

If `lazylock.json` exists but cannot be read or parsed, lazy.tmux returns an
error and aborts. It does **not** silently fall back to an empty lock file, as
doing so could overwrite a valid lock with freshly resolved commits.

---

## 7. Concurrency and Publish Model

### 7.1 Global Operation Lock

All mutating operations require the global exclusive lock:

- `init` (when writes are needed)
- `sync`
- `install`
- `update`
- `restore`
- `clean`

`list` does not require the lock.

Lock file location:

```text
$XDG_STATE_HOME/lazytmux/operations.lock
```

The lock uses OS-level `flock(LOCK_EX)` and is released when the file
descriptor is closed.

**Init lock scope**: `init` acquires the lock at entry (blocking) and holds it
through scanning, mutation, and plugin loading. This eliminates TOCTOU races
and prevents another writer from modifying plugin directories while init is
loading them.

### 7.2 Staging

All remote plugin revision switches are prepared in a staging directory first.
To ensure the publish protocol can rely on same-filesystem `rename`, `plugins/`,
`.staging/`, and `.backup/` are under the same XDG data root:

```text
{data_dir}/plugins/
{data_dir}/.staging/
{data_dir}/.backup/
```

### 7.3 Publish Protocol

#### Fresh Install

When the target directory does not exist:

1. `rename(staging, target)`
2. Execute `build` in the target directory (if declared and revision changed)
3. On build failure: remove the failed target directory

#### Replace Existing Plugin

When the target directory already exists:

1. `rename(target, backup)`
2. `rename(staging, target)`
3. Execute `build` in the target directory (if declared and revision changed)
4. On success: remove backup
5. On failure: remove failed target, `rename(backup, target)` (rollback)

This is not a lock-free atomic operation, but under the global operation lock
it is safe for lazy.tmux's own reads and writes.

### 7.4 Lock File Commit Timing

- A plugin's lock entry is updated only after its directory is successfully
  published and built.
- After all plugins are processed, the lock file is atomically written.
- Partially failed runs: successful plugins update their entries, failed ones
  retain previous values. The command returns a non-zero exit code.

### 7.5 Build Failure Markers

When a `build` command fails in the final directory, lazy.tmux records a
failure marker containing:

- Plugin ID
- Target commit
- Build command hash (SHA-256)
- Build command string
- Failure timestamp
- stderr summary

**Suppression key for init auto-retry**:

```text
(plugin-id, commit, build-command-hash)
```

This check happens inside the install path, after the candidate commit is
resolved (by cloning and resolving tracking), but before the publish step. This
ensures the exact three-part tuple is used even when no lock entry exists yet
(first-install failure case).

Semantics:

- `init` encountering a matching failure marker: logs a warning, skips the
  plugin.
- Explicit `install` / `update` / `restore`: always retries regardless of
  markers.
- Successful build: clears all failure markers for that plugin.
- Changed `build` command: produces a new build-command-hash, treated as a new
  attempt.
- Install/update/restore failures all write markers with the same structure.

**Last-result display** uses a broader match: any uncleared marker for
`(plugin-id, build-hash)` — ignoring commit — means `last-result=build-failed`.
This correctly surfaces both fresh-install failures (no lock entry) and
update/restore failures (marker commit differs from lock commit). Successful
operations clear all markers, so any remaining marker indicates the last
operation failed.

---

## 8. TPM Compatibility Contract

### 8.1 Supported Behavior

| Contract | Requirement |
|----------|-------------|
| Environment variable | `tmux set-environment -g TMUX_PLUGIN_MANAGER_PATH "{plugin_root}/"` with trailing slash |
| Option setting | `tmux set -g @key "value"` before sourcing |
| Execution scope | All `*.tmux` files in the plugin directory |
| Intra-plugin order | `*.tmux` files sorted lexicographically by filename |
| Inter-plugin order | Serial execution in `lazy.kdl` declaration order |
| Local plugins | Same opt application and `*.tmux` execution |

Note: `TMUX_PLUGIN_MANAGER_PATH` points to lazy.tmux's plugin root, which uses
the full-path layout, not TPM's flat layout.

### 8.2 Explicitly Unsupported Behavior

1. **TPM flat layout dependencies**: assuming direct children of
   `TMUX_PLUGIN_MANAGER_PATH` are plugin directories, enumerating plugins via
   `ls`, or deriving peer plugin paths by basename.
2. **TPM repo or helper scripts**: calling TPM's internal shell helpers,
   detecting the TPM repo, or assuming `~/.tmux/plugins/tpm/` exists.
3. **TPM keybinding workflows**: `prefix + I`, `prefix + U`, or TPM's
   clean/update prompts.

### 8.3 Practical Meaning

If a plugin treats TPM as "a loader that sets tmux options and executes
`*.tmux` files," it will likely work. If a plugin treats TPM as "a platform
with a specific directory layout and internal helpers," compatibility is not
guaranteed.

---

## 9. tmux Integration

Users add to `.tmux.conf`:

```tmux
run-shell "lazytmux init"
```

This is the only required integration point. lazy.tmux handles environment
setup, option application, and plugin loading within the `init` command.

---

## 10. Directory Structure

```text
~/.config/tmux/lazy.kdl                 # user configuration
~/.config/tmux/lazylock.json            # resolved lock snapshot (check into version control)

~/.local/share/lazytmux/
  +-- plugins/                          # installed plugin checkouts
  |   +-- github.com/tmux-plugins/tmux-sensible/
  |   +-- github.com/catppuccin/tmux/
  |   +-- gitlab.com/user/plugin/
  +-- .staging/                         # staging area (same filesystem as plugins)
  +-- .backup/                          # publish rollback area (same filesystem)

~/.local/state/lazytmux/
  +-- operations.lock                   # global operation lock
  +-- failures/                         # build failure markers (JSON)
```

Config file search order:

1. `$LAZY_TMUX_CONFIG`
2. `$XDG_CONFIG_HOME/tmux/lazy.kdl`
3. `~/.config/tmux/lazy.kdl`
4. `~/.tmux/lazy.kdl`

The active `lazylock.json` is always stored next to the resolved config file.
For example, `LAZY_TMUX_CONFIG=/path/to/custom.kdl` uses
`/path/to/lazylock.json`.

---

## 11. Technology Stack

| Component | Choice | Crate |
|-----------|--------|-------|
| Configuration | KDL | `kdl` |
| Lock snapshot | JSON | `serde_json` |
| CLI | clap derive | `clap` |
| Async runtime | tokio | `tokio` |
| Git operations | shell out | `tokio::process::Command` (async), `std::process::Command` (sync for scan) |
| Error handling | -- | `anyhow` + `thiserror` |
| File locking | OS flock | `fd-lock` |
| XDG paths | -- | `etcetera` |
| Hashing | SHA-256 | `sha2` |

---

## 12. Project Structure

```text
lazytmux/
+-- Cargo.toml
+-- src/
|   +-- main.rs              # CLI entry: dispatch to init/sync/install/update/restore/clean/list
|   +-- config.rs            # KDL configuration parsing and validation
|   +-- model.rs             # Config, Options, PluginSpec, PluginSource, Tracking
|   +-- planner.rs           # Compute init decision, plugin statuses, scan installed state
|   +-- plugin.rs            # install/update/restore/clean/list core workflows
|   +-- sync.rs              # Config-driven sync diffing, policies, reconcile engine
|   +-- git.rs               # clone/fetch/checkout/publish (async + sync)
|   +-- loader.rs            # Build tmux load plan: set-environment, set-option, run-shell
|   +-- lockfile.rs          # lazylock.json read/write and fingerprint helpers
|   +-- state.rs             # Paths, OperationLock, failure markers
|   +-- tmux.rs              # TmuxCommand enum and execution
+-- tests/
|   +-- config_parse.rs      # Configuration parsing and validation
|   +-- example_config.rs    # Real example config round-trip
|   +-- source_normalization.rs  # URL -> ID derivation
|   +-- planner.rs           # Init decision, status computation, failure detection
|   +-- init_flow.rs         # Init planning, lock contention, failure suppression
|   +-- operations.rs        # install/update/restore/clean/list behavior
|   +-- lockfile.rs          # Lock snapshot round-trip and version checks
|   +-- sync.rs              # Incremental sync behavior
|   +-- sync_fingerprint.rs  # Lock-affecting config fingerprinting
|   +-- loader.rs            # Load plan generation and ordering
|   +-- publish.rs           # Publish protocol: fresh install, replace, rollback
|   +-- state.rs             # Failure markers, operation lock, paths
|   +-- cli_help.rs          # CLI help output
|   +-- cli_list.rs          # CLI list output formatting
|   +-- cli_sync.rs          # Sync CLI behavior
+-- examples/
    +-- lazy.kdl             # Example configuration
```

---

## 13. Roadmap

### Phase 1: Core Engine (done)

- [x] KDL configuration parsing and validation
- [x] URL -> ID path derivation
- [x] Planner: config + lock + installed state -> target state
- [x] Init planner: config + lock + disk -> write plan
- [x] Staging + publish protocol with rollback
- [x] Lock file generation, reading, and atomic update
- [x] Global operation lock
- [x] Build failure marker mechanism
- [x] CLI output (`list` with state, last-result, current/lock commits)

### Phase 2: tmux Integration (done)

- [x] `init` command with lock-through-load flow
- [x] Option application (`set -g @...`)
- [x] `*.tmux` loading in declaration order
- [x] `TMUX_PLUGIN_MANAGER_PATH` setup
- [x] `install` / `update` / `restore` / `clean` commands
- [x] Partial failure reporting with non-zero exit codes

### Phase 3: Polish

- [ ] `migrate` command
- [ ] Concurrent git operations (currently serial)
- [ ] `list --json` structured output
- [ ] Crash recovery: stale staging cleanup on startup

### Phase 4: Future Extensions

- [ ] Hook system
- [ ] Conditional loading
- [ ] Explicit dependency declarations
- [ ] Plugin templates / scaffolding
