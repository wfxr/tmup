# Real tmux Progress Manual Checklist

Use this as a quick, real-tmux smoke pass. Do not treat it as exhaustive.

## One-time setup

```bash
cargo build --bin tmup

tmp_root="$(mktemp -d)"
work="$tmp_root/work"
mkdir -p "$work/repo-src" "$tmp_root/remotes/example.com/test"

# Local remote used for offline success-path sync/install, exposed through
# git URL rewrite so tmup still sees a normal remote URL.
printf '# demo plugin script\n' > "$work/repo-src/init.tmux"
git -C "$work/repo-src" init -b main
git -C "$work/repo-src" add init.tmux
git -C "$work/repo-src" -c commit.gpgsign=false commit -m "init"
git clone --bare "$work/repo-src" "$tmp_root/remotes/example.com/test/plugin.git"

cat > "$tmp_root/.gitconfig" <<EOF
[url "file://$tmp_root/remotes/example.com/"]
    insteadOf = https://example.com/
EOF

cat > "$work/tmup-success.kdl" <<EOF
plugin "https://example.com/test/plugin.git" name="demo"
EOF

cat > "$work/tmup-fail.kdl" <<EOF
plugin "https://example.com/test/missing.git" name="missing"
EOF
```

## Checklist

- [ ] Export the env first in the pane you will test from:

```bash
export HOME="$tmp_root"
export TMUP_CONFIG="$work/tmup-success.kdl"
export XDG_DATA_HOME="$work/data-success"
export XDG_STATE_HOME="$work/state-success"
```

- [ ] Popup success path (tmux >= 3.2): in an attached tmux client, run `./target/debug/tmup init`. Expect popup UI, successful completion, and clean return to the original pane.
- [ ] Popup failure path (tmux >= 3.2): switch `TMUP_CONFIG`/`XDG_*` to the `tmup-fail.kdl` paths, then run `./target/debug/tmup init`. Expect popup UI plus failure summary (non-zero result).
- [ ] Split fallback path (tmux 2.0-3.1 environment): run the same success command under tmux 2.x/3.1 and confirm init opens with `split-window` (not popup) and still completes.
- [ ] Cursor restoration after live progress: after a popup/split run finishes, verify the shell cursor is visible and prompt editing works normally in the original pane (no hidden cursor / broken line positioning).
- [ ] Details tail rendering: on the failure run above, confirm a `Details` line appears near the end with a log path.
- [ ] Warning tail rendering: force log-open warning, then rerun and confirm a trailing `Warning` line. Example:

```bash
mkdir -p "$work/state-warn/tmup/logs"
chmod 500 "$work/state-warn/tmup/logs"
export TMUP_CONFIG="$work/tmup-fail.kdl"
export XDG_DATA_HOME="$work/data-warn"
export XDG_STATE_HOME="$work/state-warn"
./target/debug/tmup init
```

If popup child behavior in your shell/tmux setup does not honor inline env
prefixes reliably, prefer `export ...` in the pane before running `tmup init`
rather than one-shot `VAR=... command` prefixes.
