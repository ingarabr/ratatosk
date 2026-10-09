//! Runs a full-screen child inside a pseudo-terminal that is mirrored onto this one, byte for
//! byte, while Ratatosk's own alternate screen and raw mode stay on. The main screen is never
//! shown between the two programs, and Ratatosk gets its own way back: a detach key that is
//! taken out of the input stream before the child sees it.

use std::{
    io::{self, Write, stdout},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command},
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use ratatui::{
    DefaultTerminal,
    crossterm::{
        cursor,
        event::{
            DisableFocusChange, EnableFocusChange, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{Clear, ClearType},
    },
};

const CTRL_Z: u8 = 0x1a;
const CTRL_RBRACKET: u8 = 0x1d;
const LEAVE_ALT_SCREEN: &[u8] = b"\x1b[?1049l";
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
// Modes a child that was hung up on may leave behind: mouse, focus, paste, synchronized
// output, a hidden cursor and an open SGR.
const RESET_MODES: &[u8] =
    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[?2026l\x1b[?25h\x1b[0m";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Detached,
    Ended,
}

/// Hands this terminal to `cmd` until the child exits, `done` says it can go, or Ctrl+Z or
/// Ctrl+] is pressed. `keyboard` says whether this terminal took Ratatosk's kitty keyboard
/// flags, which are popped around the child and pushed back after.
pub fn run(
    terminal: &mut DefaultTerminal,
    cmd: Command,
    done: impl Fn() -> bool,
    keyboard: bool,
) -> Result<Outcome> {
    let mut out = stdout();
    if keyboard {
        execute!(out, PopKeyboardEnhancementFlags)?;
    }
    execute!(
        out,
        DisableFocusChange,
        cursor::MoveTo(0, 0),
        Clear(ClearType::All),
        cursor::Show
    )?;

    let mut output = Output::default();
    let outcome = mirror(cmd, &done, &mut output);

    let mut reset = RESET_MODES.to_vec();
    for _ in 0..output.pushes {
        reset.extend_from_slice(b"\x1b[<u");
    }
    out.write_all(&reset)?;
    if keyboard {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    execute!(out, EnableFocusChange)?;
    terminal.clear()?;
    outcome
}

fn mirror(cmd: Command, done: &dyn Fn() -> bool, output: &mut Output) -> Result<Outcome> {
    let size = window_size(libc::STDIN_FILENO);
    let (mut child, master) = spawn(cmd, size).context("could not run `claude`")?;
    let handler = on_winch as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t;
    let previous = unsafe { libc::signal(libc::SIGWINCH, handler) };
    let outcome = proxy(
        libc::STDIN_FILENO,
        libc::STDOUT_FILENO,
        master.as_raw_fd(),
        done,
        output,
    );
    unsafe {
        libc::signal(libc::SIGWINCH, previous);
    }
    end(master, &mut child);
    outcome
}

static RESIZED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: libc::c_int) {
    RESIZED.store(true, Ordering::Relaxed);
}

fn window_size(fd: RawFd) -> Option<libc::winsize> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } == 0).then_some(size)
}

// The child becomes the session leader of its own PTY, so job control and Ctrl+Z inside it
// never reach Ratatosk, and its stdio is the PTY whatever the caller set.
fn spawn(mut cmd: Command, size: Option<libc::winsize>) -> io::Result<(Child, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = size;
    let winp = size.as_mut().map_or(null_mut(), |s| s as *mut _);
    if unsafe { libc::openpty(&mut master, &mut slave, null_mut(), null_mut(), winp) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    for fd in [&master, &slave] {
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let slave_fd = slave.as_raw_fd();
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 || libc::ioctl(slave_fd, libc::TIOCSCTTY.into(), 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            for fd in 0..3 {
                if libc::dup2(slave_fd, fd) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    drop(slave);
    Ok((child, master))
}

// Copies bytes both ways until the child's side of the PTY closes, `done` says so, or the
// input holds a detach key. Terminal queries and their replies pass through untouched, so
// the real terminal answers the child directly.
fn proxy(
    tty_in: RawFd,
    tty_out: RawFd,
    master: RawFd,
    done: &dyn Fn() -> bool,
    output: &mut Output,
) -> Result<Outcome> {
    let mut input = Input::default();
    let mut buf = [0u8; 65536];
    let mut fds = [
        libc::pollfd {
            fd: tty_in,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: master,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        if RESIZED.swap(false, Ordering::Relaxed)
            && let Some(size) = window_size(tty_in)
        {
            unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &size) };
        }
        for fd in &mut fds {
            fd.revents = 0;
        }
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, 100) } == -1 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err).context("waiting on the terminal");
        }
        if fds[1].revents != 0 {
            match read(master, &mut buf) {
                Ok(0) => {
                    write_all(tty_out, &output.finish())?;
                    return Ok(Outcome::Ended);
                }
                Ok(n) => write_all(tty_out, &output.feed(&buf[..n]))?,
                // EIO is how a PTY master reports that the child closed its side.
                Err(err) if err.raw_os_error() == Some(libc::EIO) => {
                    write_all(tty_out, &output.finish())?;
                    return Ok(Outcome::Ended);
                }
                Err(err) if retryable(&err) => {}
                Err(err) => return Err(err).context("reading from `claude`"),
            }
        }
        if fds[0].revents != 0 {
            match read(tty_in, &mut buf) {
                Ok(0) => return Ok(Outcome::Detached),
                Ok(n) => {
                    if input.detaches(&buf[..n]) {
                        return Ok(Outcome::Detached);
                    }
                    write_all(master, &buf[..n])?;
                }
                Err(err) if retryable(&err) => {}
                Err(err) => return Err(err).context("reading the terminal"),
            }
        }
        if done() {
            return Ok(Outcome::Ended);
        }
    }
}

// Closing the master hangs the child up, which ends `claude attach` and the agents view it
// turns into after ←; the latter ignores SIGTERM. The session keeps running in the daemon.
fn end(master: OwnedFd, child: &mut Child) {
    drop(master);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

fn retryable(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

fn write_all(fd: RawFd, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
        if n == -1 {
            let err = io::Error::last_os_error();
            if retryable(&err) {
                continue;
            }
            return Err(err);
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

/// Watches the keys typed for a detach key, in both the legacy byte and the kitty keyboard
/// protocol's encoding, since the child decides which one the terminal uses. Pasted text is
/// never a detach.
#[derive(Default)]
struct Input {
    in_paste: bool,
}

impl Input {
    fn detaches(&mut self, chunk: &[u8]) -> bool {
        let mut hit = false;
        let mut rest = chunk;
        while !rest.is_empty() {
            if self.in_paste {
                match find(rest, PASTE_END) {
                    Some(at) => {
                        self.in_paste = false;
                        rest = &rest[at + PASTE_END.len()..];
                    }
                    None => break,
                }
            } else {
                let typed = find(rest, PASTE_START);
                let segment = &rest[..typed.unwrap_or(rest.len())];
                hit |= is_detach(segment);
                match typed {
                    Some(at) => {
                        self.in_paste = true;
                        rest = &rest[at + PASTE_START.len()..];
                    }
                    None => break,
                }
            }
        }
        hit
    }
}

fn is_detach(typed: &[u8]) -> bool {
    typed.contains(&CTRL_Z)
        || typed.contains(&CTRL_RBRACKET)
        || kitty_keys(typed).any(|(key, mods, event)| {
            (key == u32::from(b'z') || key == u32::from(b']')) && mods & 4 != 0 && event != 3
        })
}

// Every `CSI key[:alt...];mods[:event] u` in `typed`, as (key, modifier bits, event type).
fn kitty_keys(typed: &[u8]) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
    typed
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w == b"\x1b[")
        .filter_map(move |(i, _)| {
            let body = &typed[i + 2..];
            let end = body.iter().position(|&b| b == b'u')?;
            let body = std::str::from_utf8(&body[..end]).ok()?;
            let (key, mods) = body.split_once(';')?;
            let key = key.split(':').next()?.parse().ok()?;
            let mut mods = mods.split(':');
            let bits: u32 = mods.next()?.parse().ok()?;
            let event = mods.next().and_then(|e| e.parse().ok()).unwrap_or(1);
            Some((key, bits.saturating_sub(1), event))
        })
}

/// Shapes the child's output for a terminal that stays on Ratatosk's alternate screen: the
/// child's own leave is dropped, and its kitty keyboard pushes are counted so the ones a
/// hung-up child never popped can be.
#[derive(Default)]
struct Output {
    carry: Vec<u8>,
    pushes: usize,
}

enum Csi {
    Complete(usize),
    Incomplete,
    Other,
}

impl Output {
    fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                out.push(data[i]);
                i += 1;
                continue;
            }
            let rest = &data[i..];
            match csi(rest) {
                Csi::Incomplete => {
                    self.carry = rest.to_vec();
                    break;
                }
                Csi::Other => {
                    out.push(0x1b);
                    i += 1;
                }
                Csi::Complete(len) => {
                    let seq = &rest[..len];
                    if seq == LEAVE_ALT_SCREEN {
                    } else if is_kitty(seq, b'>') {
                        self.pushes += 1;
                        out.extend_from_slice(seq);
                    } else if is_kitty(seq, b'<') {
                        self.pushes = self.pushes.saturating_sub(1);
                        out.extend_from_slice(seq);
                    } else {
                        out.extend_from_slice(seq);
                    }
                    i += len;
                }
            }
        }
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        let carry = std::mem::take(&mut self.carry);
        if LEAVE_ALT_SCREEN.starts_with(&carry) {
            Vec::new()
        } else {
            carry
        }
    }
}

fn csi(rest: &[u8]) -> Csi {
    if rest.len() < 2 {
        return Csi::Incomplete;
    }
    if rest[1] != b'[' {
        return Csi::Other;
    }
    for (j, &b) in rest[2..].iter().enumerate() {
        match b {
            0x20..=0x3f => {}
            0x40..=0x7e => return Csi::Complete(j + 3),
            _ => return Csi::Other,
        }
    }
    if rest.len() > 32 {
        Csi::Other
    } else {
        Csi::Incomplete
    }
}

// `CSI > flags u` pushes kitty keyboard flags, `CSI < n u` pops.
fn is_kitty(seq: &[u8], marker: u8) -> bool {
    seq.len() >= 4
        && seq[2] == marker
        && seq[seq.len() - 1] == b'u'
        && seq[3..seq.len() - 1].iter().all(u8::is_ascii_digit)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn detach_keys_in_both_encodings() {
        let mut input = Input::default();
        assert!(input.detaches(b"\x1a"));
        assert!(input.detaches(b"\x1d"));
        assert!(input.detaches(b"\x1b[122;5u"));
        assert!(input.detaches(b"\x1b[93;5u"));
        assert!(input.detaches(b"\x1b[122;5:2u"), "a repeat counts");
        assert!(!input.detaches(b"\x1b[122;5:3u"), "a release does not");
        assert!(!input.detaches(b"\x1b[122;1u"), "plain z");
        assert!(!input.detaches(b"\x1b[122;3u"), "alt+z");
        assert!(!input.detaches(b"z"));
        assert!(!input.detaches(b"\x1b[A"));
    }

    #[test]
    fn pasted_text_never_detaches() {
        let mut input = Input::default();
        assert!(!input.detaches(b"\x1b[200~a\x1ab\x1b[201~"));
        assert!(!input.detaches(b"\x1b[200~first"));
        assert!(!input.detaches(b"\x1a still pasting"));
        assert!(!input.detaches(b"\x1b[201~"));
        assert!(input.detaches(b"\x1a"));
        assert!(input.detaches(b"\x1b[200~x\x1b[201~\x1a"));
    }

    #[test]
    fn leaving_the_alternate_screen_is_dropped_even_split_across_reads() {
        let mut output = Output::default();
        assert_eq!(output.feed(b"a\x1b[?1049lb"), b"ab");
        assert_eq!(output.feed(b"c\x1b[?10"), b"c");
        assert_eq!(output.feed(b"49ld"), b"d");
        assert_eq!(output.feed(b"\x1b[?1049h\x1b[2J"), b"\x1b[?1049h\x1b[2J");
        assert_eq!(output.feed(b"e\x1b"), b"e");
        assert_eq!(output.feed(b"]0;title\x07"), b"\x1b]0;title\x07");
        assert_eq!(output.feed(b"\x1b[?10"), b"");
        assert_eq!(output.finish(), b"");
    }

    #[test]
    fn unfinished_output_that_is_not_the_leave_is_kept() {
        let mut output = Output::default();
        assert_eq!(output.feed(b"x\x1b[3"), b"x");
        assert_eq!(output.finish(), b"\x1b[3");
    }

    #[test]
    fn kitty_pushes_are_counted() {
        let mut output = Output::default();
        output.feed(b"\x1b[>1u\x1b[>5u\x1b[<u\x1b[>4m\x1b[<35;1;1M");
        assert_eq!(output.pushes, 1);
        output.feed(b"\x1b[<1u\x1b[<1u");
        assert_eq!(output.pushes, 0);
    }

    fn pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn a_child_that_exits_ends_the_proxy_with_its_output_shown() {
        let (stdin_r, _stdin_w) = pipe();
        let (stdout_r, stdout_w) = pipe();
        let (mut child, master) = spawn(sh("printf 'hi\\033[?1049l'; exit 0"), None).unwrap();
        let mut output = Output::default();
        let outcome = proxy(
            stdin_r.as_raw_fd(),
            stdout_w.as_raw_fd(),
            master.as_raw_fd(),
            &|| false,
            &mut output,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Ended);
        end(master, &mut child);
        drop(stdout_w);
        let mut shown = String::new();
        std::fs::File::from(stdout_r)
            .read_to_string(&mut shown)
            .unwrap();
        assert_eq!(shown, "hi");
    }

    #[test]
    fn a_detach_key_ends_the_proxy_and_the_child() {
        let (stdin_r, stdin_w) = pipe();
        let (_stdout_r, stdout_w) = pipe();
        let (mut child, master) = spawn(sh("cat"), None).unwrap();
        write_all(stdin_w.as_raw_fd(), b"\x1d").unwrap();
        let mut output = Output::default();
        let outcome = proxy(
            stdin_r.as_raw_fd(),
            stdout_w.as_raw_fd(),
            master.as_raw_fd(),
            &|| false,
            &mut output,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Detached);
        let started = Instant::now();
        end(master, &mut child);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn done_ends_the_proxy() {
        let (stdin_r, _stdin_w) = pipe();
        let (_stdout_r, stdout_w) = pipe();
        let (mut child, master) = spawn(sh("sleep 30"), None).unwrap();
        let mut output = Output::default();
        let outcome = proxy(
            stdin_r.as_raw_fd(),
            stdout_w.as_raw_fd(),
            master.as_raw_fd(),
            &|| true,
            &mut output,
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Ended);
        end(master, &mut child);
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn typed_bytes_reach_the_child() {
        let (stdin_r, stdin_w) = pipe();
        let (stdout_r, stdout_w) = pipe();
        let (mut child, master) = spawn(
            sh("stty -echo -onlcr; read line; printf '%s' \"$line\""),
            None,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        write_all(stdin_w.as_raw_fd(), b"hello\r").unwrap();
        let mut output = Output::default();
        proxy(
            stdin_r.as_raw_fd(),
            stdout_w.as_raw_fd(),
            master.as_raw_fd(),
            &|| false,
            &mut output,
        )
        .unwrap();
        end(master, &mut child);
        drop(stdout_w);
        let mut shown = String::new();
        std::fs::File::from(stdout_r)
            .read_to_string(&mut shown)
            .unwrap();
        assert_eq!(shown, "hello");
    }
}
