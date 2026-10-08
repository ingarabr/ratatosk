# Ratatosk requirements

Ratatosk is a terminal UI for managing Claude Code background sessions across many repos. It replaces `claude agents`. It works the same way, with grouping by project, PR tracking, worktree hygiene and more control over how sessions start.

The look-and-feel mock is a private claude.ai artifact (<https://claude.ai/artifact/Bio4QQsXkxqWGAm9EbPZNz>), kept out of the repo because it holds real session data.

## Priority

Start by doing what `claude agents` does today, then add features as they're needed. There's no versioned roadmap.

1. List sessions, attach, return, start, stop and rename. Same keys as the agents view.
2. Group sessions by org and repo. Start new sessions in the right repo.
3. Link PRs and show their status. Actions for review, rebase and IntelliJ.
4. Worktree overview and pruning.
5. Suggest a model with an optional model router.

## Layout and grouping

- A config file sets the base directory, `~/projects` by default. Ratatosk works the same wherever it's started from. Nothing specific to one person's setup belongs in the code; it goes in the config.
- The first level under the base directory is the org, e.g. `~/projects/<org>/<repo>`. It's the top level of the project menu.
- The git remote decides which repo a directory is, not the folder name. A folder can be named differently from its GitHub repo, or be a second clone of a repo already checked out elsewhere. Show the GitHub name, and keep the folder name as an alias.
- Each repo has a worktree policy:
  - `worktree`: new sessions get a worktree under `<repo>/.claude/worktrees/<name>`.
  - `main-checkout`: a new branch from `origin/main` in the main checkout.
  - Which repos use which policy is configuration, not code.
- Session names don't need a project prefix such as `billing:`, because the grouping already shows the project. Existing names are shown as they are.

## Sessions

- A session always starts at the repo root or in one of its worktrees. That's what makes the repo's skills, `.claude/settings.json`, CLAUDE.md and auto-memory load.
- Claude Code keeps auto-memory per git repo, so starting in the repo keeps memory apart between unrelated projects. No `autoMemoryDirectory` override is needed.
- Sessions started from `~/projects` get a flag. Ratatosk guesses their repo, and an action resumes them there (`claude --bg --resume` from the repo directory).
- Each row shows state, name, a PR icon with the PR number, model and age. The details pane shows working directory, branch, uncommitted changes, PR status and what the session is waiting for.
- A "needs me" view lists blocked sessions and what each one waits for.

## Keys

Ratatosk copies the agents view's keys wherever the agents view has one. The agents view docs are the reference: <https://code.claude.com/docs/en/agent-view.md>.

- Typing goes straight into the prompt line (`❯ describe a task for a new session`). There are no single-letter shortcuts.
- Enter:
  - with text in the prompt, opens the start confirmation;
  - with an empty prompt, attaches to the selected session.
- `→` attaches.
- `↑` and `↓` select. PgUp/PgDn and Home/End also work. Alt+↑/↓ jumps between groups.
- `←` moves into the project menu. `→` or Enter goes back to the list.
- Tab and Shift+Tab switch views: sessions, needs me, PRs, worktrees.
- `n:` in the prompt filters by text. `#123` filters by PR. Esc clears the prompt.
- Actions use Ctrl.
  - These match the agents view:
    - `^R` renames in place, inside the row. Claude Code has no command to rename a background session from outside (only Ctrl+R in the agents view and `/rename` inside the session), so Ratatosk keeps its own names in `~/.local/state/ratatosk/names.json`. The agents view and `claude --resume <name>` keep Claude's name, and the details band shows it beside Ratatosk's. Saving an empty name goes back to Claude's.
    - `^X` stops; a second `^X` within two seconds deletes.
    - `^S` switches grouping.
    - `^F` finds.
    - `^J` inserts a newline.
    - `^G` opens the prompt in `$EDITOR`.
    - Ctrl+Enter starts and attaches right away.
  - Ratatosk's own:
    - `^O` opens in IntelliJ.
    - `^P` checks out a PR for review.
    - `^B` starts a rebase session.
    - `^L` moves a session to its repo.
    - `^W` opens the PR on GitHub.
    - `^D` prunes merged worktrees.
  - Ctrl+A/E/U/K stay as line editing in the prompt.
- `?` with an empty prompt shows all keys.
- The key hints sit in a grid below the prompt line, in the agents view's wording ("ctrl+r to rename").

## Starting a session

- The confirmation box shows:
  - the prompt;
  - project, model and effort, each one a choice you can change;
  - the generated name;
  - where the session starts (worktree or branch);
  - the exact `claude --bg` command.
- The project follows the project menu: with a repo selected there, that repo is preselected; with an org selected, the picker opens filtered to `<org>/`, so typing a name offers a new folder in that org. With no filter, the project defaults to the selected session's repo.
- Picking a project never means stepping through every repo with the arrow keys. The project picker shows:
  - a filter line with a cursor, visible as soon as the project field has focus;
  - a "recent" section: projects sessions were last started in (kept in `~/.local/state/ratatosk/recent`), then the repos of current sessions;
  - "all projects" as a tree of orgs, collapsed by default. → opens an org, ← closes it, Enter picks a repo.
- Typing filters across all repos. The list also offers `+ new folder <org>/<name>`, and `org/name` narrows that to one org. Ratatosk creates the folder only when the session actually starts.
- Tab and Shift+Tab move between project, model and effort.
- Model routing:
  - The config's `manualModel` section lists orgs and prompt words. A prompt or project that matches one is never sent to a model router, and model and effort have to be picked by hand.
  - Any other prompt can get a router's suggestion for project, model and effort, when one is configured.
  - Your own picks always win over the suggestion.

## PRs

- A session's PRs are the ones the agents view links to it: `children` entries of kind `pr` in `~/.claude/jobs/<id>/state.json`. The agents view fills these by scanning the session's transcript for PRs it created, checked out, edited, commented on or pushed to, so one session can have several and they can be in any repo. A session with none falls back to the open or latest PR on its branch.
- PR state comes from `gh`, in the background: `gh pr list --state all` for each repo sessions run in and each repo a link points to, then `gh pr view` for linked PRs too old for that list. Lookups run at startup, on return from a session and when the window regains focus, at most once a minute.
- The list shows GitHub's octicon for the state (open, draft, merged, closed) with the PR number, or "N PRs" coloured by the most relevant state. Ghostty draws the octicons from its built-in Nerd Font symbols. The details band lists every PR with its state, plus the title when there's one.
- PR badges are terminal hyperlinks (OSC 8) to the PR on GitHub, opened with Cmd+click in Ghostty. Ratatosk doesn't capture the mouse, so text selection keeps working. In the list, only a single-PR badge links; the details band links every PR.
- The details band has a fixed height, so the list doesn't move when the selection changes.
- Show how many commits a PR is behind its base.
- Actions:
  - review checks the PR out into a worktree, or onto a branch for `main-checkout` repos;
  - IntelliJ opens the worktree with `idea <path>` as its own project;
  - rebase starts a session with "Rebase #N on main";
  - a last action opens the PR on GitHub.

## Worktrees and pruning

- Worktrees are a view of their own, not just a property of sessions. Transcripts expire after about 30 days, so many worktrees outlive their sessions.
- Each worktree gets a verdict, keep, prune or ask, with the reason:
  - PR merged or closed;
  - no session and no PR;
  - date of the last commit;
  - uncommitted changes.
- Never delete a worktree with uncommitted changes without an explicit confirm.
- Pruning lists what will be removed, with reasons, before doing anything.

## Restarts and upgrades

- Before an account switch or an upgrade, show the sessions with uncommitted work. Then use `claude respawn --all`.
- Warn when the agents view, the daemon or sessions run an older Claude Code version than the one installed.

## Data sources

- `claude agents --json --all` is the supported interface and the ground truth. Without `--all` it leaves out completed sessions, which the agents view still lists in its "Completed" group. In 2.1.292 it has id, name, cwd, kind, state, status, pid, sessionId and startedAt. The docs mention `waitingFor`, but this build doesn't output it.
- `~/.claude/sessions/<pid>.json` gives the live working directory, status and name. Watch it for changes and reconcile against `claude agents --json`.
- The docs say not to parse `~/.claude/jobs/<id>/state.json`, so use it as optional extra detail only. Its `detail` field is the one-line status the agents view shows ("awaiting go-ahead to push"), and the details pane uses it when it can be read.
- Never talk to the daemon's `control.sock`. Its protocol is private and authenticated.
- Nothing documented can send a prompt to a running session from outside it. Reaching a session means attaching.
- Attach by giving the terminal to `claude attach <id>`, run from the session's own directory. No tmux or embedded terminal is needed. Every way out leaves the session running in the background.
  - Ctrl+Z returns straight to Ratatosk. It stops the `claude attach` client, so Ratatosk ends that client and redraws the list.
  - ← and `/exit` open Claude's agents view first; Esc there returns to Ratatosk. This is deliberate in Claude Code (since v2.1.198). `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` doesn't change it (tested 2026-10-08), and no flag or setting does.
- Call `gh` and `git` as commands.

## Ground rules

- Ratatosk never pushes, never force-pushes and never creates tags. Those decisions stay with the user.
