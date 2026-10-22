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

- `accounts`: Claude Code logins pinned to orgs, e.g. `{"acme": {"configDir": "~/.claude-acme", "email": "me@acme.example", "orgs": ["acme"]}}`. Each account is its own `CLAUDE_CONFIG_DIR`, with its own login, sessions, transcripts and folder trust. Every session in a repo of those orgs starts, attaches and stops on that account. If the account isn't logged in, or is logged in under another `email`, Ratatosk refuses to start rather than fall back to the default login. Log in once with `CLAUDE_CONFIG_DIR=~/.claude-acme claude auth login --email me@acme.example`. Orgs follow `orgAliases`. Every other repo uses the `CLAUDE_CONFIG_DIR` Ratatosk was started with, usually none.
  Every other repo uses the default login. Ctrl+A switches it between the accounts in Ratatosk's pool: the current login joins by itself, and "sign in to another account" adds more (it runs `claude auth login` without touching the current login). Running sessions on the default login follow within about 30 seconds. The stored logins are kept in the Keychain.
- `baseDir`: the folder holding `<org>/<repo>` checkouts. Defaults to `~/projects`. `RATATOSK_BASE_DIR` overrides it.
- `manualModel`: `orgs` and `promptWords` for work where model and effort must be picked by hand. Nothing is preselected for them, and their prompts are never sent to a model router.
- `openers`: tools Ctrl+O can open a session's folder with (its worktree, or the repo root). Each has a `name`, a `command` in which `{dir}` becomes the folder, and optional `when` file patterns (`*` matches anything) checked against the folder's top level; without `when` a tool is always offered. One fitting tool opens straight away, several open a chooser, and the details band lists the ones that fit. Without `openers`, only the system file manager is offered.
- `orgAliases`: folder or GitHub org names mapped to the org they should be grouped under, e.g. `{"acme": "acme-corp"}` so repos in `~/projects/acme` without a GitHub remote join the `acme-corp` group. `manualModel.orgs` follows the aliases.
