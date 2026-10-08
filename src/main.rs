mod agents;
mod app;
mod config;
mod git;
mod launch;
mod names;
mod open;
mod picker;
mod place;
mod pr;
mod recent;
mod state;
mod ui;

use std::{
    io::stdout,
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
    let mut app = App::new(base, config.manual_model.clone(), config.openers());
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
                Action::Start { launch, open } => match launch.start() {
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
                    Err(err) => {
                        if let Some(draft) = app.draft.as_mut() {
                            draft.notice = Some(format!("{err:#}"));
                        }
                    }
                },
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
                            tilde_path(&dir),
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

// Run from the session's own directory: from anywhere else Claude Code asks to trust the
// caller's folder before it wakes the session.
fn attach(terminal: &mut DefaultTerminal, id: &str, dir: &Path) -> Result<()> {
    restore();
    let waited = Command::new("claude")
        .args(["attach", id])
        .current_dir(dir)
        .spawn()
        .context("could not run `claude attach`")
        .and_then(|child| wait_detaching_on_stop(child.id() as libc::pid_t));
    *terminal = init()?;
    terminal.clear()?;
    waited
}

// Ctrl+Z in an attached session stops the `claude attach` client, and only a shell would
// ever resume it. Treat a stop as leaving the session: end the client, the session itself
// keeps running in the daemon.
fn wait_detaching_on_stop(pid: libc::pid_t) -> Result<()> {
    loop {
        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) } == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err).context("waiting for `claude attach`");
        }
        if !libc::WIFSTOPPED(status) {
            return Ok(());
        }
        unsafe {
            libc::kill(pid, libc::SIGTERM);
            libc::kill(pid, libc::SIGCONT);
        }
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

fn tilde_path(dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if dir.starts_with(&home) => format!("~{}", &dir[home.len()..]),
        _ => dir.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    #[expect(clippy::zombie_processes, reason = "wait_detaching_on_stop reaps it")]
    fn a_child_that_stops_itself_is_ended() {
        let child = Command::new("sh")
            .args(["-c", "kill -STOP $$; sleep 30"])
            .spawn()
            .unwrap();
        let started = Instant::now();
        wait_detaching_on_stop(child.id() as libc::pid_t).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
