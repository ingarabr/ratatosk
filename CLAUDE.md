# Ratatosk

A Rust TUI (ratatui) that replaces `claude agents`. `docs/requirements.md` holds what it must do and the decisions made so far. The look-and-feel target is a private claude.ai artifact, https://claude.ai/artifact/Bio4QQsXkxqWGAm9EbPZNz. It holds real session data, so it stays out of the repo; republish to that same URL when it changes.

- The toolchain is pinned in `rust-toolchain.toml`. Run `cargo clippy` and `cargo fmt` before committing.
- Work in the main checkout. No worktrees: `.claude/settings.json` turns off background-session worktree isolation for this repo.
- Read Claude Code state only through `claude agents --json` and `~/.claude/sessions/*.json`. Never use the daemon's control socket.
- Keys follow the agents view; check https://code.claude.com/docs/en/agent-view.md before adding one.
- Nothing specific to one person's setup goes in the code, docs, tests or commit messages: no employer, client or private project names. Use config for it, and neutral names (acme, globex) in tests.
- Commit locally; push only when Ingar says so.
