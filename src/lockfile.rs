use std::{
    ffi::CString,
    fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};

// Claude Code holders touch every 5 s; touching faster keeps a margin.
const TOUCH_EVERY: Duration = Duration::from_millis(2500);
// A takeover holds its side directory for one stat and one rmdir; older than this, its holder died.
const TAKEOVER_STALE: Duration = Duration::from_secs(5);

// A lock in the format of npm's `proper-lockfile`, which Claude Code uses for its credential and
// config locks: the lock is a directory, `mkdir` is the mutex, and the holder keeps the
// directory's mtime fresh. A lock whose mtime is older than `stale` has a dead holder and may be
// taken over. Released (removed) on drop.
pub struct DirLock {
    path: PathBuf,
    // A takeover replaces the directory; acting on a replacement would compromise its holder.
    ino: u64,
    stop: Option<Sender<()>>,
    toucher: Option<JoinHandle<()>>,
}

impl DirLock {
    pub fn acquire(path: &Path, stale: Duration, timeout: Duration) -> Result<Self> {
        let start = Instant::now();
        loop {
            match fs::create_dir(path) {
                Ok(()) => break,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("could not lock {}", path.display()));
                }
            }
            if age(path).is_some_and(|age| age > stale) && remove_if_stale(path, stale) {
                continue;
            }
            if start.elapsed() >= timeout {
                bail!(
                    "{} is held by another process (Claude Code refreshing its sign-in, most likely); try again in a moment",
                    path.display()
                );
            }
            thread::sleep(Duration::from_millis(200 + jitter_ms(200)));
        }
        let ino = fs::metadata(path)
            .with_context(|| format!("could not read {}", path.display()))?
            .ino();
        let (stop, stopped) = mpsc::channel::<()>();
        let touched = path.to_path_buf();
        let toucher = thread::spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(TOUCH_EVERY) {
                if !is(&touched, ino) || touch(&touched).is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            path: path.to_path_buf(),
            ino,
            stop: Some(stop),
            toucher: Some(toucher),
        })
    }

    // False once someone else took the lock over as stale; nothing may be written after that.
    pub fn still_held(&self) -> bool {
        is(&self.path, self.ino)
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(toucher) = self.toucher.take() {
            let _ = toucher.join();
        }
        if self.still_held() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

fn is(path: &Path, ino: u64) -> bool {
    fs::metadata(path).is_ok_and(|m| m.ino() == ino)
}

fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

fn touch(path: &Path) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: `path` is a valid NUL-terminated string, and a null `times` means "now".
    if unsafe { libc::utimes(path.as_ptr(), std::ptr::null()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// Two waiters that both saw the same dead holder must not both remove the lock: the second rmdir
// would hit the first one's fresh lock. The side directory serializes takeovers, and staleness is
// checked again inside it. Claude Code never looks at the side directory.
fn remove_if_stale(path: &Path, stale: Duration) -> bool {
    let mut side = path.as_os_str().to_owned();
    side.push(".takeover");
    let side = PathBuf::from(side);
    if fs::create_dir(&side).is_err() {
        if age(&side).is_some_and(|age| age > TAKEOVER_STALE) {
            let _ = fs::remove_dir(&side);
        }
        return false;
    }
    let removed = age(path).is_some_and(|age| age > stale) && fs::remove_dir(path).is_ok();
    let _ = fs::remove_dir(&side);
    removed
}

fn jitter_ms(max: u64) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    u64::from(nanos) % max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ratatosk-lock-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_age(path: &Path, age: Duration) {
        let when = SystemTime::now() - age;
        fs::File::open(path).unwrap().set_modified(when).unwrap();
    }

    #[test]
    fn a_held_lock_blocks_until_timeout_and_is_removed_on_drop() {
        let dir = temp("held");
        let path = dir.join("x.lock");
        let lock = DirLock::acquire(&path, Duration::from_secs(60), Duration::ZERO).unwrap();
        assert!(path.is_dir());
        assert!(lock.still_held());
        let err = DirLock::acquire(&path, Duration::from_secs(60), Duration::from_millis(300));
        assert!(err.is_err(), "a fresh lock is never taken over");
        drop(lock);
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let dir = temp("stale");
        let path = dir.join("x.lock");
        fs::create_dir(&path).unwrap();
        set_age(&path, Duration::from_secs(61));
        let lock =
            DirLock::acquire(&path, Duration::from_secs(60), Duration::from_secs(2)).unwrap();
        assert!(age(&path).unwrap() < Duration::from_secs(5));
        assert!(!dir.join("x.lock.takeover").exists());
        drop(lock);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_holder_keeps_the_lock_fresh() {
        let dir = temp("fresh");
        let path = dir.join("x.lock");
        let lock = DirLock::acquire(&path, Duration::from_secs(10), Duration::ZERO).unwrap();
        set_age(&path, Duration::from_secs(9));
        thread::sleep(TOUCH_EVERY + Duration::from_millis(500));
        assert!(age(&path).unwrap() < Duration::from_secs(3));
        drop(lock);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_lock_taken_over_by_someone_else_is_no_longer_held() {
        let dir = temp("lost");
        let path = dir.join("x.lock");
        let lock = DirLock::acquire(&path, Duration::from_secs(60), Duration::ZERO).unwrap();
        fs::remove_dir(&path).unwrap();
        assert!(!lock.still_held());
        fs::create_dir(&path).unwrap();
        assert!(
            !lock.still_held(),
            "a new lock at the same path is someone else's"
        );
        drop(lock);
        assert!(path.is_dir(), "dropping never removes someone else's lock");
        fs::remove_dir_all(dir).unwrap();
    }
}
