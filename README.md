# Ratatosk

A terminal UI for Claude Code background sessions across many repos. It replaces `claude agents` with grouping by org and repo, PR status, worktree pruning and more control over where and how sessions start.

Named after the squirrel that runs up and down Yggdrasil carrying messages between its levels.

## Status

Early. It lists Claude Code background sessions grouped by repo, attaches to them, starts new ones and stops them. What it should become is in [`docs/requirements.md`](docs/requirements.md).

## Run

```sh
cargo run
```

`rust-toolchain.toml` pins the Rust version; rustup installs it on first build.

It needs `claude` (Claude Code), `gh` and `git` on `PATH`.

## Configuration

Ratatosk reads `~/.config/ratatosk/config.json` (or `$XDG_CONFIG_HOME/ratatosk/config.json`, or the file `RATATOSK_CONFIG` points to). Every setting is optional, and unknown keys are an error; see [`docs/config.example.json`](docs/config.example.json).

- `baseDir`: the folder holding `<org>/<repo>` checkouts. Defaults to `~/projects`. `RATATOSK_BASE_DIR` overrides it.
- `manualModel`: `orgs` and `promptWords` for work where model and effort must be picked by hand. Nothing is preselected for them, and their prompts are never sent to a model router.
