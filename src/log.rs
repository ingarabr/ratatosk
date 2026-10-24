//! An append-only log of what went wrong, and of each session start, at
//! `~/.local/state/ratatosk/ratatosk.log`. It holds sizes and outcomes, never prompt text or
//! tokens. Logging never fails the caller: a log that can't be written is skipped.

use std::{
    fmt::Display,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

const ROTATE_AT: u64 = 1024 * 1024;

pub fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::state::dir().map(|dir| dir.join("ratatosk.log"))
}

/// Logs `err` as the reason `what` failed, and gives back the text to show for it.
pub fn failed(what: &str, err: &anyhow::Error) -> String {
    let shown = format!("{err:#}");
    write("error", &format!("{what}: {shown}"));
    shown
}

pub fn error(what: &str, err: impl Display) {
    write("error", &format!("{what}: {err}"));
}

pub fn info(message: impl Display) {
    write("info", &message.to_string());
}

/// Moves a log past `ROTATE_AT` to `ratatosk.log.1`, so it never grows without bound.
pub fn rotate() {
    let Some(path) = path() else {
        return;
    };
    if fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_AT) {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }
}

/// Logs a panic before the terminal is restored and the message is lost with the screen.
pub fn on_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write("panic", info);
        previous(info);
    }));
}

fn write(level: &str, message: &dyn Display) {
    let Some(path) = path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line(&now(), level, &message.to_string()).as_bytes());
    }
}

// Continuation lines are indented, so every entry starts at a timestamp.
fn line(at: &str, level: &str, message: &str) -> String {
    format!(
        "{at} {level:<5} {}\n",
        message.trim_end().replace('\n', "\n    ")
    )
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return secs.to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_one_line_with_its_continuations_indented() {
        assert_eq!(
            line(
                "2026-10-24 14:00:00",
                "error",
                "start: failed\nstderr: boom\n"
            ),
            "2026-10-24 14:00:00 error start: failed\n    stderr: boom\n"
        );
    }

    #[test]
    fn the_time_is_local_and_readable() {
        let at = now();
        assert_eq!(at.len(), 19, "{at}");
        assert_eq!(&at[4..5], "-");
    }
}
