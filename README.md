<h1 align="center">lazytmux</h1>

<p align="center">
  A modern, lock-first tmux plugin manager — inspired by <a href="https://github.com/folke/lazy.nvim">lazy.nvim</a>.
</p>

<p align="center">
  <a href="#features">Features</a> &bull;
  <a href="#installation">Installation</a> &bull;
  <a href="#quick-start">Quick Start</a> &bull;
  <a href="#configuration">Configuration</a> &bull;
  <a href="#commands">Commands</a> &bull;
  <a href="#tpm-compatibility">TPM Compatibility</a>
</p>

---

## Why lazytmux?

[TPM](https://github.com/tmux-plugins/tpm) has been the de-facto tmux plugin
manager for years, but it is largely unmaintained and carries several structural
limitations: pure bash implementation, weak error handling, serial
install/update, no lock file, and no interactive UI.

lazytmux is a ground-up rewrite in Rust that brings the convenience of
lazy.nvim's design philosophy to tmux:

- **Declarative config** — a single `lazy.kdl` file describes everything.
- **Lock file** — `lazylock.json` pins exact commits for reproducible setups.
- **Concurrent operations** — installs and updates run in parallel.
- **Safe publish protocol** — staging + atomic rename + rollback on build failure.
- **Interactive TUI** — a keyboard-driven dashboard as the primary interface.

## Features

- **Lock-first** — `lazylock.json` is the source of truth. Only `update`
  advances versions; `init`, `install`, and `restore` always respect the lock.
- **Safe publish** — every revision change goes through a staging directory
  first. Build failures trigger automatic rollback to the previous version.
- **Writer-aware init** — `init` does a read-only preflight. If another writer
  is active, it waits rather than reading a half-written plugin directory.
- **Build failure memory** — failed builds are recorded as
  `(plugin, commit, build-command-hash)` tuples. `init` won't auto-retry the
  same failure. Change the build command or run `install` explicitly to retry.
- **TPM-compatible** — plugins that use `@option` + `*.tmux` entry scripts work
  out of the box.
- **TUI + CLI** — interactive dashboard for day-to-day use; CLI subcommands for
  scripting and automation.

## Installation

### From source

```bash
cargo install --path .
```

### Pre-built binaries

Coming soon.

## Quick Start

**1. Create a config file**

```bash
mkdir -p ~/.config/tmux
cat > ~/.config/tmux/lazy.kdl << 'EOF'
options {
    auto-install #true
}

plugin "tmux-plugins/tmux-sensible"
plugin "tmux-plugins/tmux-yank"
plugin "catppuccin/tmux" opt-prefix="catppuccin_" {
    opt "flavor" "mocha"
}
EOF
```

**2. Add to `.tmux.conf`**

```tmux
# Load plugins on startup
run-shell "lazytmux init"

# Open TUI with prefix + L (tmux 3.2+)
bind L display-popup -E -w 80% -h 80% "lazytmux"
```

**3. Reload tmux**

```bash
tmux source-file ~/.tmux.conf
```

lazytmux will auto-install missing plugins on the first `init` and generate
`lazylock.json`. Commit the lock file to version control for reproducible
setups across machines.

## Configuration

lazytmux uses [KDL v2](https://kdl.dev) syntax. Config file search order:

1. `$LAZY_TMUX_CONFIG`
2. `$XDG_CONFIG_HOME/tmux/lazy.kdl`
3. `~/.config/tmux/lazy.kdl`
4. `~/.tmux/lazy.kdl`

### Full example

```kdl
options {
    concurrency 8
    auto-install #true
    auto-clean #false
    bind-ui #false
    ui-key "L"
}

// GitHub shorthand — track default branch
plugin "tmux-plugins/tmux-sensible"

// Pin to a tag — update skips pinned plugins
plugin "tmux-plugins/tmux-yank" tag="v2.3"

// Branch + build command + options
plugin "tmux-plugins/tmux-resurrect" branch="master" build="make install" {
    opt "resurrect-strategy-vim" "session"
    opt "resurrect-save-bash-history" "on"
}

// opt-prefix avoids repetition: opt "flavor" → @catppuccin_flavor
plugin "catppuccin/tmux" opt-prefix="catppuccin_" {
    opt "flavor" "mocha"
    opt "window_text" "#W"
}

// Non-GitHub source
plugin "https://gitlab.com/user/my-plugin.git"

// Local plugin — loaded in-place, not in lock file
plugin "~/dev/my-tmux-plugin" local=#true name="my-plugin-dev"

// Disable with KDL slashdash
/-plugin "tmux-plugins/tmux-continuum"
```

### Options reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `concurrency` | int | `8` | Max parallel git operations |
| `auto-install` | bool | `#true` | Install missing plugins during `init` |
| `auto-clean` | bool | `#false` | Remove undeclared plugins during `init` |
| `bind-ui` | bool | `#false` | Auto-register TUI keybinding |
| `ui-key` | string | `"L"` | Key for the TUI binding |

### Plugin properties

| Property | Type | Default | Description |
|----------|------|---------|-------------|
| 1st arg | string | — | GitHub `user/repo`, full git URL, or local path |
| `name` | string | basename of id | Display name for TUI/logs |
| `opt-prefix` | string | `""` | Prefix prepended to all `opt` keys |
| `branch` | string | — | Track a specific branch |
| `tag` | string | — | Pin to a tag (update skips) |
| `commit` | string | — | Pin to a commit (update skips) |
| `local` | bool | `#false` | Treat source as a local path |
| `build` | string | — | Shell command to run after install/update |

> `branch`, `tag`, and `commit` are mutually exclusive.

### Option mechanism

Each `opt "key" "value"` child becomes:

```
tmux set -g @{opt-prefix}{key} "{value}"
```

## Commands

```
lazytmux                    # Open interactive TUI
lazytmux init               # Startup path: install missing, apply opts, load plugins
lazytmux install [id]       # Install missing remote plugins
lazytmux update [id]        # Update remote plugins (only command that advances lock)
lazytmux restore [id]       # Restore to lock-recorded commits
lazytmux clean              # Remove undeclared managed plugins
lazytmux list               # Print plugin status table
lazytmux migrate            # Migrate from TPM declarations (WIP)
```

### `init` — startup path

Designed for `run-shell "lazytmux init"` in `.tmux.conf`. Optimized for speed:

1. **Read-only preflight** — parse config, read lock, scan installed plugins.
2. **If everything is aligned** — skip locking, just set options and source
   `*.tmux` files. Near-zero overhead.
3. **If plugins are missing** — acquire exclusive lock, replan (another process
   may have already installed), then install and load.
4. **If a writer is active** — wait for it to finish, then re-preflight.

`init` never updates existing plugins, never changes existing lock entries,
and never retries a known build failure automatically.

### `update` — advance versions

The **only** command that writes new commits to the lock file.

| Tracking | Behavior |
|----------|----------|
| branch / default | Fetch and advance to latest remote commit |
| `tag="..."` | Skip, report `pinned-tag` |
| `commit="..."` | Skip, report `pinned-commit` |

### `list` — status overview

Outputs a table with separated **state** and **last-result** columns:

| State | Meaning |
|-------|---------|
| `installed` | Plugin present and matches lock |
| `missing` | Declared but not on disk |
| `pinned-tag` | Installed, pinned to a tag |
| `pinned-commit` | Installed, pinned to a commit |

| Last Result | Meaning |
|-------------|---------|
| `ok` | Last operation succeeded |
| `build-failed` | Build command failed (marker recorded) |
| `none` | No operation attempted yet |

## TUI

Launch with `lazytmux` (no subcommand) or via a tmux keybinding:

```
╭──────────────────── lazy.tmux ─────────────────────╮
│                                                     │
│  Installed 12   Updates 3   Missing 1   Pinned 2   │
│                                                     │
│  ✓ tmux-sensible      remote  installed   abc1234   │
│  ↻ resurrect          remote  outdated    def5678   │
│  ! catppuccin/tmux    remote  missing     -------   │
│  • my-plugin-dev      local   local       -------   │
│                                                     │
│  I install  U update  C clean  R restore  / search  │
│  l log  d diff  x remove  ? help  q quit            │
╰─────────────────────────────────────────────────────╯
```

Navigation: `j`/`k` or arrow keys. `q` to quit.

## Directory Layout

```
~/.config/tmux/
  ├── lazy.kdl                          # configuration
  └── lazylock.json                     # lock file (commit to VCS)

~/.local/share/lazytmux/
  ├── plugins/                          # installed plugins
  │   ├── github.com/tmux-plugins/tmux-sensible/
  │   ├── github.com/catppuccin/tmux/
  │   └── gitlab.com/user/plugin/
  ├── .staging/                         # in-progress installs
  └── .backup/                          # rollback during publish

~/.local/state/lazytmux/
  ├── operations.lock                   # cross-process mutex
  └── failures/                         # build failure markers
```

Plugin directories use the full `host/owner/repo` path (like Go modules) to
avoid basename collisions between `user1/tmux-foo` and `user2/tmux-foo`.

## TPM Compatibility

lazytmux is compatible with the majority of TPM plugins — specifically those
that work through:

- `tmux set -g @...` options
- `*.tmux` entry scripts
- `TMUX_PLUGIN_MANAGER_PATH` environment variable

### Not compatible

Plugins that depend on TPM internals will **not** work:

- Assuming `TMUX_PLUGIN_MANAGER_PATH` has a flat `plugin-name/` layout
- Calling TPM's internal shell helpers
- Detecting the TPM repo at `~/.tmux/plugins/tpm/`
- Relying on TPM keybindings (`prefix + I`, `prefix + U`)

This boundary is intentional, not an oversight.

## Migrating from TPM

1. Create `~/.config/tmux/lazy.kdl` based on your `set -g @plugin` lines.
2. Replace the TPM `run` line in `.tmux.conf` with `run-shell "lazytmux init"`.
3. Restart tmux. lazytmux will clone all plugins fresh and generate the lock file.
4. Commit `lazy.kdl` and `lazylock.json` to your dotfiles repo.
5. Remove the old `~/.tmux/plugins/` directory when satisfied.

## License

MIT
