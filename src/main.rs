mod account;
mod agents;
mod app;
mod carried;
mod config;
mod edit;
mod git;
mod handoff;
mod identity;
mod keychain;
mod launch;
mod lockfile;
mod names;
mod open;
mod picker;
mod place;
mod pool;
mod pr;
mod recent;
mod state;
mod trust;
mod ui;

use std::{
    io::stdout,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste,
            EnableFocusChange, Event, KeyEventKind, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
};

use account::Account;
use app::{Action, App, Line, Move};

fn main() -> Result<()> {
    let config = config::Config::load()?;
    let base = config.base_dir()?;
    let accounts = config.accounts()?;
    let pool = std::env::var_os("HOME").map(|home| {
        pool::Pool::new(
            Path::new(&home),
            pool::Pool::index_path(),
            keychain::user(),
            accounts.pinned_emails(),
        )
    });
    let mut app = App::new(base, &config, accounts, pool);
    app.request_prs(Instant::now());
    if std::env::args().nth(1).as_deref() == Some("--list") {
        while app.prs_pending() {
            app.receive_prs();
            std::thread::sleep(Duration::from_millis(50));
        }
        return print_list(app);
    }
    app.sync_pool();
    let mut terminal = init()?;
    let result = run(&mut terminal, app);
    restore();
    result
}

fn init() -> Result<DefaultTerminal> {
    let terminal = ratatui::init();
    execute!(stdout(), EnableFocusChange, EnableBracketedPaste)?;
    if keyboard_enhanced() {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    Ok(terminal)
}

// Asked once: the answer is a round trip through the terminal, and it must not be asked
// while a child owns the input.
fn keyboard_enhanced() -> bool {
    static ENHANCED: OnceLock<bool> = OnceLock::new();
    *ENHANCED.get_or_init(|| supports_keyboard_enhancement().unwrap_or(false))
}

fn restore() {
    if keyboard_enhanced() {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableFocusChange, DisableBracketedPaste);
    ratatui::restore();
}

fn run(terminal: &mut DefaultTerminal, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        app.receive_prs();
        // Poll only while a PR lookup is out, so its results can redraw the list.
        let timeout = if app.prs_pending() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(3600)
        };
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::FocusGained => {
                app.refresh();
                app.request_prs(Instant::now());
            }
            Event::Paste(text) => app.on_paste(&text),
            Event::Key(key) if key.kind == KeyEventKind::Press => match app.on_key(key) {
                Action::Quit => return Ok(()),
                Action::Attach { id, dir, account } => {
                    let account = app.accounts.get(account).clone();
                    if trust::is_trusted(&account, &dir) == Some(false) {
                        run_claude_in(terminal, &account, &dir)?;
                    }
                    if trust::is_trusted(&account, &dir) == Some(false) {
                        app.status = Some(format!(
                            "the {} account doesn't trust {} yet, so the session can't open there",
                            account.name,
                            place::tilde(&dir)
                        ));
                        continue;
                    }
                    attach(terminal, &account, &id, &dir)?;
                    app.refresh();
                    app.request_prs(Instant::now());
                }
                Action::Start { launch, open } => start(terminal, &mut app, &launch, open)?,
                Action::Trust { launch, open } => {
                    let account = app.accounts.get(launch.account).clone();
                    if let Err(err) = logged_in(&account) {
                        if let Some(draft) = app.draft.as_mut() {
                            draft.notice = Some(format!("{err:#}"));
                        }
                        continue;
                    }
                    if launch.create {
                        let _ = std::fs::create_dir_all(&launch.dir);
                    }
                    run_claude_in(terminal, &account, &launch.dir)?;
                    start(terminal, &mut app, &launch, open)?;
                }
                Action::Move(planned) => {
                    app.status = Some(match move_session(terminal, &mut app, &planned) {
                        Ok(()) => format!(
                            "moved {} to the {} account",
                            planned.name,
                            app.accounts.get(planned.to).name
                        ),
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::SwitchAccount(email) => {
                    app.status = Some(match switch_account(&mut app, &email) {
                        Ok(message) => message,
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::SignIn => {
                    app.status = Some(match sign_in(terminal, &mut app) {
                        Ok(message) => message,
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::Stop { id, account } => {
                    app.status = Some(match agents::stop(app.accounts.get(account), &id) {
                        Ok(_) => {
                            app.armed_delete = Some((id.clone(), std::time::Instant::now()));
                            format!("stopped {id} · ctrl+x again within 2s to delete it")
                        }
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::Delete { id, account } => {
                    app.status = Some(match agents::remove(app.accounts.get(account), &id) {
                        Ok(out) if out.is_empty() => format!("deleted {id}"),
                        Ok(out) => out,
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::Open { opener, dir } => {
                    app.status = Some(match app.openers[opener].launch(&dir) {
                        Ok(()) => format!(
                            "opened {} in {}",
                            place::tilde(&dir),
                            app.openers[opener].name
                        ),
                        Err(err) => format!("{err:#}"),
                    });
                }
                Action::None => {}
            },
            _ => {}
        }
    }
}

fn start(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    launch: &launch::Launch,
    open: bool,
) -> Result<()> {
    let refused = |app: &mut App, message: String| {
        if let Some(draft) = app.draft.as_mut() {
            draft.needs_trust = true;
            draft.notice = Some(message);
        }
    };
    let account = app.accounts.get(launch.account).clone();
    if let Err(err) = logged_in(&account) {
        if let Some(draft) = app.draft.as_mut() {
            draft.notice = Some(format!("{err:#}"));
        }
        return Ok(());
    }
    let folder = place::tilde(&launch.dir);
    let ask = format!(
        "Claude Code doesn't trust {folder} yet. Enter opens Claude there to answer its trust prompt; once you accept, Ratatosk closes it and starts this session."
    );
    if !launch.create && trust::is_trusted(&account, &launch.dir) == Some(false) {
        refused(app, ask);
        return Ok(());
    }
    match launch.start(&account) {
        Ok(stdout) => {
            app.remember(&launch.label);
            app.input.clear();
            app.draft = None;
            app.refresh();
            if let Some((id, cwd)) = app.select_started(&stdout)
                && open
            {
                attach(terminal, &account, &id, &cwd)?;
                app.refresh();
            }
        }
        Err(err) if trust::is_trust_error(&format!("{err:#}")) => refused(app, ask),
        Err(err) => {
            if let Some(draft) = app.draft.as_mut() {
                draft.notice = Some(format!("{err:#}"));
            }
        }
    }
    Ok(())
}

// An interactive `claude` in the folder, only so its trust prompt can be answered: it is closed
// as soon as the folder is trusted, and the caller then starts the real session.
fn run_claude_in(terminal: &mut DefaultTerminal, account: &Account, dir: &Path) -> Result<()> {
    let mut cmd = account.claude();
    cmd.current_dir(dir);
    handoff::run(
        terminal,
        cmd,
        || trust::is_trusted(account, dir) == Some(true),
        keyboard_enhanced(),
    )?;
    Ok(())
}

// Run from the session's own directory: from anywhere else Claude Code asks to trust the
// caller's folder before it wakes the session. Ctrl+Z or Ctrl+] comes back here.
fn attach(terminal: &mut DefaultTerminal, account: &Account, id: &str, dir: &Path) -> Result<()> {
    let mut cmd = account.claude();
    cmd.args(["attach", id]).current_dir(dir);
    handoff::run(terminal, cmd, || false, keyboard_enhanced())?;
    Ok(())
}

fn switch_account(app: &mut App, email: &str) -> Result<String> {
    let pool = app
        .pool
        .as_mut()
        .context("account switching isn't available")?;
    pool.switch(&keychain::Keychain, email)?;
    let now = app.accounts.get(account::DEFAULT).signed_in_as()?;
    ensure!(
        now.as_deref()
            .is_some_and(|now| now.eq_ignore_ascii_case(email)),
        "switched, but Claude Code reports {} as signed in; check `claude auth status`",
        now.as_deref().unwrap_or("nobody")
    );
    Ok(format!(
        "signed in as {email} · running sessions on the default login follow within 30 s"
    ))
}

// Signs in under a throwaway CLAUDE_CONFIG_DIR, so the default login and its refresh token are
// never touched; the new login is then copied into the pool.
fn sign_in(terminal: &mut DefaultTerminal, app: &mut App) -> Result<String> {
    let dir = state::dir().context("HOME is not set")?.join("sign-in");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut cmd = std::process::Command::new("claude");
    cmd.env("CLAUDE_CONFIG_DIR", &dir)
        .args(["auth", "login"])
        .current_dir(&app.base);
    let ran = handoff::run(terminal, cmd, || false, keyboard_enhanced());
    let imported = app
        .pool
        .as_mut()
        .context("account switching isn't available")
        .and_then(|pool| pool.import(&keychain::Keychain, &dir));
    let _ = std::fs::remove_dir_all(&dir);
    ran?;
    let member = imported?;
    Ok(format!(
        "added {} · enter on it in ctrl+a makes it the default login",
        member.email
    ))
}

// A pinned account must be logged in as itself: starting there must never fall back to another
// login.
fn logged_in(account: &Account) -> Result<()> {
    if account.pinned() {
        account.check_login()?;
    }
    Ok(())
}

// Stops the session, copies its transcript to the pinned account and resumes it there from the
// same folder. The old session is removed only once the new one runs.
fn move_session(terminal: &mut DefaultTerminal, app: &mut App, planned: &Move) -> Result<()> {
    let from = app.accounts.get(planned.from).clone();
    let to = app.accounts.get(planned.to).clone();
    to.check_login()?;
    ensure!(
        planned.cwd.is_dir(),
        "{} is gone, so the session can't resume there",
        place::tilde(&planned.cwd)
    );
    if planned.running {
        agents::stop(&from, &planned.id)?;
    }
    account::copy_transcript(&from, &to, &planned.session_id)?;
    if trust::is_trusted(&to, &planned.cwd) != Some(true) {
        run_claude_in(terminal, &to, &planned.cwd)?;
    }
    let out = to
        .claude()
        .args(["--bg", "--resume", &planned.session_id])
        .current_dir(&planned.cwd)
        .output()
        .context("could not run `claude --bg --resume`")?;
    ensure!(
        out.status.success(),
        "`claude --bg --resume` failed on the {} account: {}",
        to.name,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // `claude rm` also deletes a worktree Claude created for the session, which the resumed
    // session still runs in. Such an old entry is left stopped; Ratatosk hides it.
    app.carry_prs(&planned.session_id, &planned.prs);
    if !planned.in_worktree {
        agents::remove(&from, &planned.id)?;
    }
    app.refresh();
    if let Some(new) = app
        .sessions
        .iter()
        .find(|s| s.account == planned.to && s.session_id.as_deref() == Some(&planned.session_id))
        .map(|s| s.id.clone())
    {
        app.carry_name(&planned.id, &new);
    }
    Ok(())
}

fn print_list(app: App) -> Result<()> {
    if let Some(err) = &app.error {
        anyhow::bail!("{err}");
    }
    for line in &app.lines {
        match line {
            Line::Header(group) => println!("{group}"),
            Line::Session(i) => {
                let session = &app.sessions[*i];
                let worktree = app.places[*i]
                    .worktree()
                    .map(|w| format!("  [{w}]"))
                    .unwrap_or_default();
                let pr: String = app
                    .prs_for(*i)
                    .iter()
                    .map(|pr| format!("  #{} {:?}", pr.number, pr.state))
                    .collect();
                let account = app
                    .misplaced(*i)
                    .map(|want| format!("  ⚠ not on {}", app.accounts.get(want).name))
                    .unwrap_or_default();
                println!(
                    "  {} {}{worktree}{pr}{account}",
                    session.state.glyph(),
                    session.name
                );
            }
        }
    }
    Ok(())
}
