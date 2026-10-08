mod agents;
mod app;
mod config;
mod git;
mod identity;
mod launch;
mod names;
mod open;
mod picker;
mod place;
mod pr;
mod recent;
mod state;
mod trust;
mod ui;

use std::{
    io::stdout,
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableFocusChange, EnableFocusChange, Event, KeyEventKind,
            KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
};

use app::{Action, App, Line};

fn main() -> Result<()> {
    let config = config::Config::load()?;
    let base = config.base_dir()?;
    let mut app = App::new(base, &config);
    app.request_prs(Instant::now());
    if std::env::args().nth(1).as_deref() == Some("--list") {
        while app.prs_pending() {
            app.receive_prs();
            std::thread::sleep(Duration::from_millis(50));
        }
        return print_list(app);
    }
    let mut terminal = init()?;
    let result = run(&mut terminal, app);
    restore();
    result
}

fn init() -> Result<DefaultTerminal> {
    let terminal = ratatui::init();
    execute!(stdout(), EnableFocusChange)?;
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    Ok(terminal)
}

fn restore() {
    if supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableFocusChange);
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
            Event::Key(key) if key.kind == KeyEventKind::Press => match app.on_key(key) {
                Action::Quit => return Ok(()),
                Action::Attach { id, cwd } => {
                    let dir = if cwd.is_dir() { cwd } else { app.base.clone() };
                    attach(terminal, &id, &dir)?;
                    app.refresh();
                    app.request_prs(Instant::now());
                }
                Action::Start { launch, open } => start(terminal, &mut app, &launch, open)?,
                Action::Trust { launch, open } => {
                    if launch.create {
                        let _ = std::fs::create_dir_all(&launch.dir);
                    }
                    run_claude_in(terminal, &launch.dir)?;
                    start(terminal, &mut app, &launch, open)?;
                }
                Action::Stop { id } => {
                    app.status = Some(match agents::stop(&id) {
                        Ok(_) => {
                            app.armed_delete = Some((id.clone(), std::time::Instant::now()));
                            format!("stopped {id} · ctrl+x again within 2s to delete it")
                        }
                        Err(err) => format!("{err:#}"),
                    });
                    app.refresh();
                }
                Action::Delete { id } => {
                    app.status = Some(match agents::remove(&id) {
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
    let folder = place::tilde(&launch.dir);
    let ask = format!(
        "Claude Code doesn't trust {folder} yet. Enter opens Claude there to answer its trust prompt; once you accept, Ratatosk closes it and starts this session."
    );
    if !launch.create && trust::is_trusted(&launch.dir) == Some(false) {
        refused(app, ask);
        return Ok(());
    }
    match launch.start() {
        Ok(stdout) => {
            app.remember(&launch.label);
            app.input.clear();
            app.draft = None;
            app.refresh();
            if let Some((id, cwd)) = app.select_started(&stdout)
                && open
            {
                attach(terminal, &id, &cwd)?;
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
fn run_claude_in(terminal: &mut DefaultTerminal, dir: &Path) -> Result<()> {
    let mut cmd = Command::new("claude");
    cmd.current_dir(dir);
    run_in_terminal(terminal, cmd, || trust::is_trusted(dir) == Some(true))
}

// Run from the session's own directory: from anywhere else Claude Code asks to trust the
// caller's folder before it wakes the session.
fn attach(terminal: &mut DefaultTerminal, id: &str, dir: &Path) -> Result<()> {
    let mut cmd = Command::new("claude");
    cmd.args(["attach", id]).current_dir(dir);
    run_in_terminal(terminal, cmd, || false)
}

// Hands the terminal to `cmd` until it exits, is suspended, or `done` says it can go.
//
// Ctrl+Z in an interactive `claude` suspends the whole foreground job, which includes
// Ratatosk, so Ratatosk ignores SIGTSTP while the child runs; the child gets the default
// action back before exec, so its own Ctrl+Z still works.
fn run_in_terminal(
    terminal: &mut DefaultTerminal,
    mut cmd: Command,
    done: impl Fn() -> bool,
) -> Result<()> {
    restore();
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
        cmd.pre_exec(|| {
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            Ok(())
        });
    }
    let waited = cmd
        .spawn()
        .context("could not run `claude`")
        .and_then(|child| wait_for(child.id() as libc::pid_t, done));
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
    }
    *terminal = init()?;
    terminal.clear()?;
    waited
}

// A suspended child is ended rather than left stopped: only a shell would ever resume it, and
// for `claude attach` the session itself keeps running in the daemon.
fn wait_for(pid: libc::pid_t, done: impl Fn() -> bool) -> Result<()> {
    let end = || unsafe {
        libc::kill(pid, libc::SIGTERM);
        libc::kill(pid, libc::SIGCONT);
    };
    let mut ending = false;
    loop {
        let mut status = 0;
        let got = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
        if got == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err).context("waiting for `claude`");
        }
        if got == pid && !libc::WIFSTOPPED(status) {
            return Ok(());
        }
        if (got == pid || done()) && !ending {
            end();
            ending = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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
                println!("  {} {}{worktree}{pr}", session.state.glyph(), session.name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    #[expect(clippy::zombie_processes, reason = "wait_for reaps it")]
    fn a_child_that_stops_itself_is_ended() {
        let child = Command::new("sh")
            .args(["-c", "kill -STOP $$; sleep 30"])
            .spawn()
            .unwrap();
        let started = Instant::now();
        wait_for(child.id() as libc::pid_t, || false).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
