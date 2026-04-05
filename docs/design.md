# tmup Design

## Context

TPM (tmux plugin manager) has been unmaintained for years. It suffers from
structural limitations: pure bash implementation, weak error handling, serial
install/update, no lock file, and no reproducible state management.

tmup is a Rust-based tmux plugin manager inspired by lazy.nvim's design
philosophy: concise configuration, concurrent operations, reproducible
environments, and safe staged publish semantics.

The project focuses exclusively on CLI-driven workflows. The core value is
config-driven sync, safe staged publish, lock-through-load init, and
script-friendly behavior with reliable exit codes.

---

## 1. Design Goals and Non-Goals

### Goals

1. **Reproducible**: the same `tmup.kdl` + `tmup.lock` snapshot produces
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
   but never advances existing plugin versions. Known-failed build tuples are
   still recorded, and init-mode implicit sync suppresses exact-tuple retries
   until the `(plugin-id, commit, build-command-hash)` changes.
5. **No full-screen TUI in MVP**: tmup supports a live terminal progress view
   for long-running operations, but does not provide an interactive full-screen
   TUI.
6. **No hooks, registry, or dependency resolution in MVP**: these belong to
   future extensions.
7. **No support for out-of-band filesystem manipulation inside the managed
   plugin root**: manually cloned repos, in-place repo edits, and symlink-based
   layouts under `plugin_root` are outside the current contract.

---

## 2. Core Principles

| Principle | Description |
|-----------|-------------|
| Config-driven sync | `tmup.kdl` is the desired state for remote plugins; `tmup.lock` is the resolved snapshot that mutating commands sync first. |
| Prepare concurrent, apply serial | Prepare-phase git operations (clone, fetch, resolve, stage) run in parallel with bounded concurrency. Publish, build, lock mutation, and tmux loading execute serially in declaration order. |
| URL-derived identity | Remote plugin IDs are derived from canonical source URLs, avoiding conflicts and manual naming. |
| Zero magic options | `opt-prefix` defaults to empty. No automatic prefix inference or separator insertion. |
| Compatibility by contract | Explicitly declare which TPM behaviors are supported and which are not. |
| Safe publish | New revisions are prepared in a staging directory, built there, and only then swapped into place. |
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
- The managed plugin tree is tmup-owned state; manually cloning into it,
  mutating repos in place, or introducing symlink-based layouts is unsupported.
- Cleanup is defined only for undeclared remote directories that tmup
  still recognizes as managed git repos (currently: directories under
  `plugin_root` that still contain `.git`).

Directory layout:

```text
~/.local/share/tmup/plugins/
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
- Loaded in-place by tmup.
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

## 4. Configuration Sources

tmup supports two configuration-loading modes:

- `pure`: load only `tmup.kdl`
- `mixed`: load both sources and merge them

The public mode selector is the `TMUP_CONFIG_MODE` environment variable:

```text
TMUP_CONFIG_MODE=pure|mixed
```

Default mode is `pure`. Setting `TMUP_CONFIG_MODE=mixed` selects `mixed`.

In `mixed` mode:

- remote plugins are deduplicated by canonical remote plugin ID
- `tmup.kdl` wins on conflict
- a warning is emitted when a TPM declaration is ignored because a KDL entry
  for the same remote plugin ID exists
- repeated TPM declarations for the same canonical remote plugin ID collapse to
  the first declaration
- plugin order starts from the TPM-compatible declarations discovered by tmup's
  TPM scan; KDL-only entries, including local plugins, are appended afterward
  in KDL order, while conflicting entries keep the TPM slot but use the KDL
  config
- local plugins may come only from `tmup.kdl`

### 4.1 tmup KDL

```kdl
// ~/.config/tmux/tmup.kdl

options {
    auto-install #true
    concurrency 16
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

### 4.1.1 Option Mechanism

Formula:

```text
set -g @{opt-prefix}{key} "{value}"
```

Rules:

- `opt-prefix` defaults to `""`.
- No automatic prefix inference.
- No automatic `-` or `_` separator.
- The user is responsible for the final tmux option name.

### 4.1.2 Plugin Properties

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| First argument | string | -- | GitHub `user/repo`, full git URL, or local path |
| `name` | string | remote: ID basename; local: path basename | Display name for list/logs |
| `opt-prefix` | string | `""` | Prefix prepended to opt keys |
| `branch` | string | default branch | Track a specific branch |
| `tag` | string | -- | Pinned release selector; `update` skips by default |
| `commit` | string | -- | Fixed commit; `update` skips |
| `local` | bool | `false` | Local path plugin, loaded in-place |
| `build` | string | -- | Executed in the staged plugin directory before publish |
| `opt` | child node | -- | Becomes `set -g @{opt-prefix}{key} "{value}"` |

### 4.1.3 Global Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `auto-install` | bool | `true` | Install missing plugins during `init` |
| `concurrency` | integer | `16` | Max concurrent remote prepare jobs; `1` forces serial prepare |

### 4.1.4 Validation Rules

1. Remote plugin IDs must be unique; duplicates cause an error.
2. `branch`, `tag`, `commit` are mutually exclusive.
3. `local=true` requires a path that expands to an absolute local path.
4. Remote plugins always enter the lock snapshot after successful sync; local plugins never do.
5. Local plugins cannot declare `branch` / `tag` / `commit`.
6. `concurrency` must be an integer >= 1 and fit the platform `usize`.

### 4.2 TPM-Compatible tmux Config

In `mixed` mode, tmup supplements `tmup.kdl` with plugin declarations read from
the tmux config source set.

Supported discovery order:

1. `$XDG_CONFIG_HOME/tmux/tmux.conf`
2. `~/.config/tmux/tmux.conf`
3. `~/.tmux.conf`

Supported declarations:

- `set -g @plugin 'user/repo'`
- `set -g @plugin 'user/repo#branch'`
- `set-option -g @plugin 'user/repo'`
- `set-option -g @plugin 'user/repo#branch'`
- `set -g @plugin 'https://host/user/repo.git'`
- `set -g @plugin 'git@host:user/repo.git'`

Supported `source-file` behavior intentionally matches current TPM behavior:

- tmup scans the root tmux config file
- tmup includes only directly sourced files discovered there
- tmup does not recursively expand nested sourced files
- tmup mirrors TPM's current `@plugin` extraction behavior and does not accept
  broader `set` flag combinations than TPM for plugin discovery

tmup does not infer ownership of ordinary `@foo` tmux options in TPM mode.
Those options remain tmux-managed runtime state and are consumed by plugins in
the usual way when their `*.tmux` scripts execute.

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
tmup init               # tmux startup: install missing, apply opts, load plugins
tmup sync [id]          # reconcile config into the lock snapshot
tmup install [id]       # install all/specified missing remote plugins after sync
tmup update [id]        # update unchanged floating selectors after sync
tmup restore [id]       # restore plugins to lock-recorded commits
tmup clean              # remove undeclared managed remote repos
tmup list               # list plugin status
```

The CLI target selector is the **remote plugin ID**. `name` is for display
only. Local plugins do not participate in `sync` / `install` / `update` / `restore`.
Every public command that consumes configuration also reads:

```text
TMUP_CONFIG_MODE=pure|mixed
```

### 5.2 `init` (tmux startup path)

Must be both fast and safe. The global operation lock is held for the entire
init (from scan through loading), eliminating TOCTOU races between preflight
and mutation.

```text
1. Acquire the global operation lock (blocking).
2. Load configuration according to the resolved mode (`pure` or `mixed`), read `tmup.lock`, and
   validate the effective configuration.
3. Run implicit incremental sync as the only remote-plugin reconciliation pass.
   - Install newly declared remote plugins only when auto-install=true.
   - Repair missing, broken, or drifted managed plugin repos against the lock
     snapshot.
   - Re-publish same-commit plugins when build/config metadata changed.
   - Drop removed remote plugins from the lock snapshot immediately.
   - Do not delete undeclared plugin directories here; that remains the job of
     `clean`.
4. Load plugins into tmux (set options, source *.tmux files).
5. If implicit sync recorded per-plugin failures, return non-zero after loading
   completes.
6. Release the lock.
```

Key constraints:

- `init` **never advances** unchanged floating selectors beyond what config declares.
- Exact-tuple known-failure suppression is applied inside init-mode sync just
  before publish/build, so matching `(plugin-id, commit, build-command-hash)`
  tuples are skipped without retrying the build.
- True operation-level failures still abort before tmux load; per-plugin sync
  failures load already-available plugins first, then exit non-zero.
- When all plugins are installed and lock is unchanged, init performs no git
  network access.

### 5.3 `sync [id]`

- Public command that reconciles remote plugin config into `tmup.lock`.
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
  build runs in staging before publish.
- Build failures during restore write failure markers, matching the semantics
  of install and update.
- Restore reuses cached objects when possible, but no longer promises offline
  recovery if the desired revision is not already available locally.

### 5.7 `clean`

- Runs a prune-only implicit sync first.
- Removes undeclared remote directories from the managed tree when tmup
  still recognizes them as managed git repos.
- Does not promise to clean arbitrary residue left behind by manual edits,
  symlink layouts, or broken directories that no longer match the managed-repo
  shape.
- Does not remove local plugin sources.
- Must not install, rebuild, replace, or otherwise mutate declared plugin
  directories as a side effect.
- Cleans up empty intermediate parent directories after removal.

### 5.8 `list`

Default columns:

| Column | Description |
|--------|-------------|
| `plugin` | User-facing plugin reference, based on the configured source string |
| `kind` | `remote` / `local` |
| `state` | `installed` / `missing` / `outdated` / `broken` / `pinned-tag` / `pinned-commit` / `local` |
| `last-result` | `ok` / `build-failed` / `none` |
| `current` | Installed HEAD commit (short hash or `-`) |
| `lock` | Lock-recorded commit (short hash or `-`) |

Verbose columns (`tmup list -v`):

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

- Update build failure: `state=installed`, `last-result=build-failed`
- Fresh install build failure: `state=missing`, `last-result=build-failed`

`list` does not mutate `tmup.lock` or plugin state. If the lock snapshot is
stale relative to the effective configuration for the selected
mode, it prints a warning before the table.

## 6. Lock File

### 6.1 Format

```json
{
  "version": 2,
  "config_fingerprint": "b4a0d7c2...",
  "plugins": {
    "github.com/tmux-plugins/tmux-sensible": {
      "tracking": { "type": "default-branch", "value": "main" },
      "commit": "abc1234567890abcdef1234567890abcdef1234",
      "config_hash": "c78128e1..."
    },
    "github.com/tmux-plugins/tmux-resurrect": {
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
- Cache accelerates future sync/install/update/restore operations, but does not
  guarantee offline reconstruction of arbitrary historical revisions.

### 6.3 Write Strategy

1. Serialize to `tmup.lock.tmp`.
2. `fsync`.
3. `rename` to `tmup.lock`.
4. Explicit sync and implicit sync preflights may still write updated metadata
   even when some plugins fail, preserving previous entries for failed plugins.

### 6.4 Read Error Handling

If `tmup.lock` exists but cannot be read or parsed, tmup returns an
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
$XDG_STATE_HOME/tmup/operations.lock
```

The lock uses OS-level `flock(LOCK_EX)` and is released when the file
descriptor is closed.

**Init lock scope**: `init` acquires the lock at entry (blocking) and holds it
through scanning, mutation, and plugin loading. This eliminates TOCTOU races
and prevents another writer from modifying plugin directories while init is
loading them.

### 7.2 Bounded Prepare-Phase Concurrency

All mutating operations that involve remote plugins split work into three phases:

1. **Candidate selection** (serial) — filter plugins by target, policy, and
   eligibility rules (pinned selectors, health, lock state).
2. **Parallel prepare** — clone cache repos, fetch refs, resolve tracking
   selectors, and materialize staging checkouts. At most
   `options.concurrency` (default 16) jobs run concurrently via
   `futures::stream::buffer_unordered`.
3. **Serial apply** — publish staged checkouts, run build commands, update
   failure markers, and mutate the in-memory lock — all in declaration order.

Progress events may arrive out-of-order during the prepare phase. Terminal
output and the detail log handle this correctly. Failure logs include the
canonical plugin id and processing stage for filtering:

```text
== plugin id=github.com/owner/repo name=repo stage=fetching ==
summary: git fetch origin failed
clone_url: https://github.com/owner/repo.git
```

### 7.3 Staging

All remote plugin revision switches are prepared in a staging directory first.
To ensure the publish protocol can rely on same-filesystem `rename`, `plugins/`
and `.staging/` are under the same XDG data root:

```text
{data_dir}/plugins/
{data_dir}/.staging/
```

### 7.4 Publish Protocol

#### Fresh Install

When the target directory does not exist:

1. Execute `build` in `staging` (if declared).
2. `rename(staging, target)`.
3. On build failure: discard staging and leave the target missing.

#### Replace Existing Plugin

When the target directory already exists:

1. Execute `build` in `staging` (if declared).
2. Remove the existing target directory.
3. `rename(staging, target)`.
4. On build failure: discard staging and leave the existing target untouched.
5. If the final filesystem swap itself fails, automatic rollback is not
   guaranteed.

This is not a lock-free atomic operation, but under the global operation lock
it is safe for tmup's own reads and writes.

### 7.5 Lock File Commit Timing

- A plugin's lock entry is updated only after its directory is successfully
  published and built.
- After all plugins are processed, the lock file is atomically written.
- Partially failed runs: successful plugins update their entries, failed ones
  retain previous values. The command returns a non-zero exit code.

### 7.5 Build Failure Markers

When a `build` command fails in staging, tmup records a
failure marker containing:

- Plugin ID
- Target commit
- Build command hash (SHA-256)
- Build command string
- Failure timestamp
- stderr summary

**Failure marker key**:

```text
(plugin-id, commit, build-command-hash)
```

This exact tuple is checked inside the shared sync publish path, after the
candidate commit is resolved (by cloning and resolving tracking), but before the
publish step. That keeps init-specific suppression in the same reconciliation
engine used for install/restore drift repair and same-commit rebuild decisions.

Semantics:

- `init`: matching tuples are skipped during implicit sync and surfaced as
  skipped work rather than retried builds.
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
| Inter-plugin order | Serial execution in `tmup.kdl` declaration order |
| Local plugins | Same opt application and `*.tmux` execution |

Note: `TMUX_PLUGIN_MANAGER_PATH` points to tmup's plugin root, which uses
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
TMUP_CONFIG_MODE=mixed
run-shell "tmup init"
```

This is the recommended integration point when adopting TPM compatibility,
because `tmup init` and interactive `tmup` commands inside tmux will then
agree on the same resolved mode after the tmux server is restarted. tmup
still handles environment setup, option application, and plugin loading
within the `init` command.

---

## 10. Directory Structure

```text
~/.config/tmux/tmup.kdl                 # tmup-native configuration
~/.config/tmux/tmux.conf                # optional TPM-compatible config source
~/.config/tmux/tmup.lock                # resolved snapshot when using the default tmup.kdl

~/.local/share/tmup/
  +-- plugins/                          # installed plugin checkouts
  |   +-- github.com/tmux-plugins/tmux-sensible/
  |   +-- github.com/catppuccin/tmux/
  |   +-- gitlab.com/user/plugin/
  +-- .staging/                         # staging area (same filesystem as plugins)
  +-- .repos/                           # persistent remote cache

~/.local/state/tmup/
  +-- operations.lock                   # global operation lock
  +-- failures/                         # build failure markers (JSON)
```

tmup KDL search order:

1. `$TMUP_CONFIG`
2. `$XDG_CONFIG_HOME/tmux/tmup.kdl`
3. `~/.config/tmux/tmup.kdl`

Mutating commands and `init` create the selected default `tmup.kdl` with a
minimal commented template when it does not exist yet. Read-only commands do
not create it. When `TMUP_CONFIG` is set explicitly, it must point to an
existing file.

TPM-compatible tmux config search order:

1. `$XDG_CONFIG_HOME/tmux/tmux.conf`
2. `~/.config/tmux/tmux.conf`
3. `~/.tmux.conf`

`tmup.lock` is always stored next to the active `tmup.kdl`.

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
| XDG paths | explicit environment resolution | -- |
| Hashing | SHA-256 | `sha2` |

---

## 12. Project Structure

```text
tmup/
+-- Cargo.toml
+-- src/
|   +-- main.rs              # CLI entry: dispatch to init/sync/install/update/restore/clean/list
|   +-- config.rs            # KDL configuration parsing and validation
|   +-- model.rs             # Config, Options, PluginSpec, PluginSource, Tracking
|   +-- planner.rs           # Plugin statuses and installed-state inspection helpers
|   +-- plugin.rs            # install/update/restore/clean/list core workflows
|   +-- sync.rs              # Config-driven sync diffing, policies, reconcile engine
|   +-- git.rs               # clone/fetch/checkout/publish (async + sync)
|   +-- loader.rs            # Build tmux load plan: set-environment, set-option, run-shell
|   +-- lockfile.rs          # tmup.lock read/write and fingerprint helpers
|   +-- state.rs             # Paths, OperationLock, failure markers
|   +-- tmux.rs              # TmuxCommand enum and execution
+-- tests/
|   +-- config_parse.rs      # Configuration parsing and validation
|   +-- example_config.rs    # Real example config round-trip
|   +-- source_normalization.rs  # URL -> ID derivation
|   +-- planner.rs           # Init decision, status computation, failure detection
|   +-- init_flow.rs         # Init preview, lock contention, failure suppression
|   +-- operations.rs        # install/update/restore/clean/list behavior
|   +-- lockfile.rs          # Lock snapshot round-trip and version checks
|   +-- sync.rs              # Incremental sync behavior
|   +-- sync_fingerprint.rs  # Lock-affecting config fingerprinting
|   +-- loader.rs            # Load plan generation and ordering
|   +-- publish.rs           # Publish protocol: fresh install and replace
|   +-- state.rs             # Failure markers, operation lock, paths
|   +-- cli_help.rs          # CLI help output
|   +-- cli_list.rs          # CLI list output formatting
|   +-- cli_sync.rs          # Sync CLI behavior
+-- examples/
    +-- tmup.kdl             # Example configuration
```

---

## 13. Roadmap

### Phase 1: Core Engine (done)

- [x] KDL configuration parsing and validation
- [x] URL -> ID path derivation
- [x] Planner: config + lock + installed state -> target state
- [x] Init planner: config + lock + disk -> write plan
- [x] Staging + build-before-swap publish protocol
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

- [x] Concurrent git operations (bounded prepare-phase concurrency, default 16)
- [ ] `list --json` structured output
- [ ] Crash recovery: stale staging cleanup on startup

### Phase 4: Future Extensions

- [ ] Hook system
- [ ] Conditional loading
- [ ] Explicit dependency declarations
- [ ] Plugin templates / scaffolding
