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
- Sessions are grouped by org, then repo:
  - a git repo whose `origin` is on GitHub: the GitHub owner and repo name;
  - any other folder at `<base>/<org>/<project>` (no remote, another host, or not git at all): the folder names;
  - a session directly in the base folder or an org folder: a group named after that folder (e.g. `~/projects`);
  - a session outside the base folder: "elsewhere".
- `orgAliases` in the config maps folder or GitHub org names to the org to group under, so folder-only repos can join their GitHub org's group.
- Several clones of one repo count as the same repo. The main clone, used by the new-session picker, is the folder named most like the repo.
- Icons tell group kinds apart: GitHub mark, git, folder.
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
- Ctrl+S switches the order within each group between name and last activity (newest first); the list title shows which. The age column is the time since the session was last active, taken from the job's `updatedAt` (falling back to `startedAt`, which resets on every restart), and is coloured from bright and bold under an hour, fading to grey over seven days, and dim after that.
- Groups collapse, for screen sharing among other things, with the arrow keys acting on what is selected (tree-view style). In the list, group headers are selectable: ← collapses an open group and → expands a collapsed one; → on an open group moves into it, ← on a collapsed group or on a session opens the project menu, and → on a session attaches. Enter on a header toggles it. In the project menu, ← collapses an open org (or moves from a repo to its org) and → expands a collapsed org; → on anything else returns to the list. A collapsed group shows only its header and session count. Collapsed groups and orgs, and the sort order, are remembered in `~/.local/state/ratatosk/view.json`.

## Keys

Ratatosk copies the agents view's keys wherever the agents view has one. The agents view docs are the reference: <https://code.claude.com/docs/en/agent-view.md>.

- Typing goes straight into the prompt line (`❯ describe a task for a new session`). There are no single-letter shortcuts.
- Enter:
  - with text in the prompt, opens the start confirmation;
  - with an empty prompt, attaches to the selected session.
- `→` attaches.
- `↑` and `↓` select. PgUp/PgDn and Home/End also work. Alt+↑/↓ jumps between groups.
- Tab and Shift+Tab switch between the project menu and the list. `←` on a session also opens the menu; `→` or Enter goes back to the list.
- Views (sessions, needs me, PRs, worktrees) need keys of their own, since Tab switches panels.
- `n:` in the prompt filters by text. `#123` filters by PR. Esc clears the prompt.
- Actions use Ctrl.
  - These match the agents view:
    - `^R` renames in place, inside the row. Claude Code has no command to rename a background session from outside (only Ctrl+R in the agents view and `/rename` inside the session), so Ratatosk keeps its own names in `~/.local/state/ratatosk/names.json`. The agents view and `claude --resume <name>` keep Claude's name, and the details band shows it beside Ratatosk's. Saving an empty name goes back to Claude's.
    - `^X` stops; a second `^X` within two seconds deletes.
    - `^S` switches grouping.
    - `^F` finds.
    - `^J` inserts a newline.
    - `^G` opens the prompt in `$VISUAL` or `$EDITOR` (`vi` without either), run in Ratatosk's terminal like an attached session. Collapsed pastes go to the editor in full and collapse again if they come back unchanged. A detach key leaves the editor and keeps the prompt as it was. With the cursor next to a collapsed paste, `^G` opens the paste view instead.
    - Ctrl+Enter starts and attaches right away.
  - Ratatosk's own:
    - `^O` opens the session's folder (worktree or repo root) with a tool from the config's `openers`. Tools can be limited to folders with certain files at the top (`Cargo.toml`, `pom.xml`, `bleep.yaml`); one fitting tool opens directly, several open a chooser. The details band shows which tools fit.
    - `^P` checks out a PR for review.
    - `^B` starts a rebase session.
    - `^L` moves a session to its repo, and to the account its org is pinned to.
    - `^A` switches the account of the default login.
    - `^W` opens the PR on GitHub.
    - `^D` prunes merged worktrees.
  - Line editing is the same in every text field (the prompt, the rename box, the project filter), and follows the shell's (readline's) keys where Ratatosk doesn't use them for something else. Every field has a cursor:
    - ←/→ move by character; Option+←/→ (or Ctrl+←/→, Alt+B/Alt+F) by word; Home/End, Cmd+←/→, `^A` and `^E` to the start and end of the line. In the prompt, ←/→/Home/End act on the list while the prompt is empty, and `^A` opens accounts only then. In the project filter ←/→ stay on the tree.
    - ⌫ and Delete delete a character; Option+⌫ (or Ctrl+⌫) the word before the cursor, Alt+D or Option+Delete the word after it.
    - Cmd+⌫ and `^U` delete to the start of the line, `^K` to its end; at the edge of a line they join it to the next one. Ghostty sends Cmd+⌫ as `^U` by default, so both are handled. `^Y` puts back what was deleted last.
    - `^W` is not a word delete here, since it opens the PR, and `^B`, `^D`, `^F` and `^P` keep their own jobs.
- Pasting never acts on what is pasted: Ratatosk turns on bracketed paste, so a newline in a paste is text, not Enter. A paste into the rename box or the project filter has its line breaks turned into spaces. As in the agents view, a paste longer than 800 characters or more than three lines collapses in the prompt to a `[Pasted text #N +L lines]` label, which the cursor and deletion treat as one character; the full text is sent on start. Pasting the same text again opens it up in place. The confirmation shows the label too.
- The paste view checks a collapsed paste before it is sent. `^G` with the cursor next to a paste label, or a click on the label, opens it in a box over the list, like the confirmation: the whole paste, lines numbered, long lines wrapped rather than cut. ↑↓, PgUp/PgDn, Home/End and the wheel scroll; ⌫ removes the paste from the prompt; Enter puts it in the prompt as plain text; `^G` opens it in the editor; Esc or a click outside closes it. While the cursor is next to a paste, the hint line says how to open it.
- The mouse: a click selects a session or group header, and a double click attaches or toggles the group, as Enter does. A click in the project menu picks that project. The wheel moves the selection in the list and the menu. A click in the prompt moves the cursor there. Ratatosk asks the terminal only for clicks and the wheel, never motion, and gives the mouse back while another program has the screen. Taking the mouse means the terminal's own text selection needs Shift held inside Ratatosk.
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
- Claude Code starts a background session only in a folder whose trust prompt was accepted for that exact folder (a trusted parent doesn't count; a worktree counts as its main checkout, where Claude Code files the answer), and Ratatosk never accepts it on the user's behalf. It checks the flag in `~/.claude.json` before starting, and when the folder isn't trusted (or `claude --bg` refuses), the confirmation says so: Enter then opens an interactive `claude` there to answer the prompt; Ratatosk closes it as soon as the folder is trusted and starts the session.
- The prompt line wraps and grows up to eight lines. Shift+Enter, Alt+Enter or Ctrl+J insert a newline. The confirmation shows the prompt, the command and any message wrapped in full.
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

## Accounts

- An org can be pinned to a Claude Code account in the config (`accounts`). Repos of a pinned org always use that account, never the default one. Claude Code has no account switch, so an account is a `CLAUDE_CONFIG_DIR` of its own: its own Keychain login, daemon, session list, transcripts, auto-memory and folder trust (`.claude.json` moves into the directory too).
- Every `claude` command for a pinned org runs with that account's `CLAUDE_CONFIG_DIR`: start, the trust step, attach, stop and delete. The default account leaves the variable as Ratatosk got it, since Claude Code keys the Keychain entry by the variable's literal value, and naming `~/.claude` explicitly looks logged out.
- Before starting on a pinned account, `claude auth status --json` must show it logged in, and as the configured `email` when one is set. Otherwise the confirmation shows the `claude auth login` command to run, and nothing starts.
- The list runs `claude agents --json --all` once per account and merges the results; each session remembers its account. The confirmation shows the account and the full command with `CLAUDE_CONFIG_DIR`, and the details band names a non-default account.
- A session of a pinned org running on another account is marked "⚠ not on <account>". `^L` moves it:
  1. it stops the session if it is running;
  2. it copies the transcript (and the subagents' folder) into the pinned account's `projects/`, plus the repo's auto-memory when the account has none for that folder yet;
  3. it resumes it there with `claude --bg --resume <session-id>` from the same folder, after the trust step if needed;
  4. only then does it remove the old one, except for a session in a worktree: `claude rm` also deletes the worktree Claude created for the session, which the resumed session still runs in. That old entry stays stopped on the account it left, and Ratatosk hides it while the same session runs on a pinned account. Deleting it from the agents view would still delete the worktree.
  Ratatosk's own name for the session carries over, and so do its PR links: the agents view keeps those per account, so Ratatosk saves them in `~/.local/state/ratatosk/moved-prs.json` by session id and merges them back in.
- Folder trust is per account. Attaching to a session on an account that hasn't accepted the folder's trust prompt opens Claude there first to answer it, as starting a session does. When the session's folder is gone, attach runs from the repo root instead.
- Every other repo uses the default login (`~/.claude`), and that login can be switched between several accounts: the pool. Ctrl+A opens it.
  - Enter on an account switches the default login to it. All running sessions on the default login follow within about 30 s, since Claude Code caches its Keychain read for 30 s and re-reads it on a 401; nothing needs a restart. Sessions of pinned orgs are untouched.
  - The current login joins the pool by itself. "Sign in to another account" runs `claude auth login` under a throwaway `CLAUDE_CONFIG_DIR` and copies the result into the pool, so the default login is never touched while you sign in. Never `/logout`: it revokes the refresh token.
  - Ctrl+X twice forgets an account's stored login.
  - The list title shows the account of the default login, and the start confirmation names the account a session will start on.
- A switch replaces what Claude Code reads its login from: the `claudeAiOauth` key inside the Keychain item `Claude Code-credentials` (account `$USER`), and the `oauthAccount` key of `~/.claude.json`. The rest of the Keychain item (MCP and gateway tokens) belongs to the machine and stays, as do settings, trust, history, plugins and the daemon.
  - Claude Code rotates the refresh token whenever it refreshes. The login being left is therefore saved, freshly read, at every switch, and Ratatosk saves the active one again at startup. A stored copy that went stale anyway (a switch made outside Ratatosk) needs a new sign-in.
  - It cooperates with Claude Code's locks (checked against 2.1.308), which are npm proper-lockfile directories: `~/.claude/.oauth_refresh.lock`, then `<realpath of ~/.claude>.lock`, both stale after 60 s (token refresh); `~/.claude/.storage-write.lock`, stale after 15 s (every read-modify-write of the Keychain item); and `~/.claude.json.lock`, stale after 10 s. Ratatosk takes them in that order, and like Claude Code it lets go of the first when the second is busy, so neither side waits while holding what the other needs. The storage-write and config locks are only tried, never waited on: Claude Code gives up on them after a few seconds and would lose what it meant to save. While they are held Ratatosk makes at most three `security` calls, each limited to 2 s like Claude Code's own. While held they are touched every 2.5 s. A lock is only touched or removed while it is still the same directory Ratatosk created; replacing someone else's would compromise their refresh. Ratatosk writes no `.oauth_refresh.lock.owner` record, which leaves Claude Code nothing to take over while Ratatosk is alive.
  - A switch first saves the login being left, under the refresh locks only, so no refresh can rotate it meanwhile. Then, under all four, it re-reads the live login and checks it is still the one it saved, writes the stored one and reads it back, then rewrites `~/.claude.json` atomically in Claude Code's own format (two-space indent, key order kept, no final newline, same permissions). If anything fails after the Keychain write started, the previous item is put back. A Claude Code refresh that waited on the locks finds an unexpired credential afterwards and keeps it.
  - Keychain writes go through `/usr/bin/security -i` on stdin, so tokens never appear in a process list. Stored logins live in the Keychain under the service `Ratatosk pool`, one item per email holding its `claudeAiOauth`; `~/.local/state/ratatosk/pool.json` lists the emails with their `oauthAccount`, which holds no secrets.
  - Known gap: Claude Code writes a new login to the Keychain and to `~/.claude.json` in two steps. A `/login` inside a session that lands exactly between them while Ratatosk saves the current login would file the new tokens under the old email.
  - Switching refuses when `CLAUDE_CONFIG_DIR` or `CLAUDE_SECURESTORAGE_CONFIG_DIR` is set, when the login is kept in `~/.claude/.credentials.json` or the config in `~/.claude/.config.json`, when `~/.claude.json` is a symlink, and when the login being left has no email to save it under. An account pinned to an org can't join the pool.
- Later: show each account's 5-hour and 7-day usage (`/api/oauth/usage`) in the picker, and offer to switch when a session stops on a usage limit.

## Restarts and upgrades

- Before an account switch or an upgrade, show the sessions with uncommitted work. Then use `claude respawn --all`.
- Warn when the agents view, the daemon or sessions run an older Claude Code version than the one installed.

## Data sources

- `claude agents --json --all` is the supported interface and the ground truth. Interactive sessions (a plain `claude` in another terminal) are listed too, without `id` or `state`; Ratatosk shows them but can't attach to, stop or delete them. Rows are parsed one by one, so an unexpected row is skipped rather than breaking the list. Without `--all` it leaves out completed sessions, which the agents view still lists in its "Completed" group. In 2.1.292 it has id, name, cwd, kind, state, status, pid, sessionId and startedAt. The docs mention `waitingFor`, but this build doesn't output it.
- `~/.claude/sessions/<pid>.json` gives the live working directory, status and name. Watch it for changes and reconcile against `claude agents --json`.
- The docs say not to parse `~/.claude/jobs/<id>/state.json`, so use it as optional extra detail only. Its `detail` field is the one-line status the agents view shows ("awaiting go-ahead to push"), and the details pane uses it when it can be read.
- Never talk to the daemon's `control.sock`. Its protocol is private and authenticated.
- Nothing documented can send a prompt to a running session from outside it. Reaching a session means attaching.
- Attach by running `claude attach <id>` in a pseudo-terminal that Ratatosk mirrors onto its own, byte for byte, from the session's own directory. This is what the daemon does one level down: every background session lives in a daemon-owned PTY and `claude attach` is a thin client for it. No tmux or terminal emulator is needed, since nothing is parsed; terminal queries and their replies round-trip to the real terminal. Every way out leaves the session running in the background.
  - Ratatosk stays on its alternate screen and in raw mode throughout, so the shell's scrollback never flashes between the two. The child's own leave-alternate-screen sequence is dropped from its output, and its kitty keyboard pushes are counted so the ones a hung-up child never popped can be.
  - Ctrl+Z and Ctrl+] return straight to Ratatosk. They are taken out of the input before the child sees them, in both the legacy byte and the kitty keyboard encoding, and never inside a bracketed paste. Ratatosk then closes the PTY, which ends the client; the agents view that ← turns the client into ignores SIGTERM and Ctrl+Z but ends on that hangup.
  - ← and `/exit` open Claude's agents view first; Esc there, or a detach key, returns to Ratatosk. This is deliberate in Claude Code (since v2.1.198): ← re-executes the client as `claude agents` in place. `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` doesn't change it (tested 2026-10-08), nor does `leftArrowOpensAgents: false` (tested 2026-10-09), and no flag, keybinding action or hook covers detach.
  - A mod loaded into the session does run inside `claude attach` and can swallow `/exit` through a `command.run` hook (verified 2026-10-09), so a later step can make `/exit` return to Ratatosk too. Mods cannot see keys or ←.
- Call `gh` and `git` as commands.

## Ground rules

- Ratatosk never pushes, never force-pushes and never creates tags. Those decisions stay with the user.
