use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

// An absolute path, so nothing earlier on PATH can intercept the secrets.
const SECURITY: &str = "/usr/bin/security";
// Claude Code's own limit. Every call made while Claude Code's write locks are held must be short.
const TIMEOUT: Duration = Duration::from_secs(2);
const NOT_FOUND: i32 = 44;
const LOCKED: i32 = 36;
// `security -i` reads its stdin line into a 4096-byte buffer; Claude Code switches to argv
// above this length, and so does this.
const STDIN_LINE_MAX: usize = 4032;

pub trait Secrets {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn write(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, service: &str, account: &str) -> Result<()>;
}

pub struct Keychain;

impl Secrets for Keychain {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>> {
        let (code, stdout, stderr) = run(
            &["find-generic-password", "-a", account, "-w", "-s", service],
            None,
        )?;
        match code {
            0 => Ok(Some(stdout.trim_end_matches('\n').to_string())),
            NOT_FOUND => Ok(None),
            LOCKED => bail!("the login keychain is locked"),
            _ => bail!(
                "could not read {service} from the keychain: {}",
                stderr.trim()
            ),
        }
    }

    fn write(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        let hex: String = secret.bytes().map(|b| format!("{b:02x}")).collect();
        let line = format!(
            "add-generic-password -U -a {} -s {} -X {}\n",
            quote(account),
            quote(service),
            quote(&hex)
        );
        let (code, _, stderr) = if line.len() <= STDIN_LINE_MAX {
            run(&["-i"], Some(&line))?
        } else {
            run(
                &[
                    "add-generic-password",
                    "-U",
                    "-a",
                    account,
                    "-s",
                    service,
                    "-X",
                    &hex,
                ],
                None,
            )?
        };
        match code {
            0 => Ok(()),
            LOCKED => bail!("the login keychain is locked"),
            _ => bail!(
                "could not write {service} to the keychain: {}",
                stderr.trim()
            ),
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<()> {
        let (code, _, stderr) = run(
            &["delete-generic-password", "-a", account, "-s", service],
            None,
        )?;
        match code {
            0 | NOT_FOUND => Ok(()),
            _ => bail!(
                "could not delete {service} from the keychain: {}",
                stderr.trim()
            ),
        }
    }
}

// The account name Claude Code files its credential under: $USER, else the OS user name, and
// `claude-code-user` when that isn't a plain name.
pub fn user() -> String {
    let name = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(os_user);
    match name {
        Some(name)
            if name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) =>
        {
            name
        }
        _ => "claude-code-user".to_string(),
    }
}

fn os_user() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage, or null; it is read at once.
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() || (*pw).pw_name.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

// `security -i` splits each line shell-style.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn run(args: &[&str], stdin: Option<&str>) -> Result<(i32, String, String)> {
    let mut child = Command::new(SECURITY)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run /usr/bin/security")?;
    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        pipe.write_all(input.as_bytes())
            .context("could not write to /usr/bin/security")?;
    }
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "/usr/bin/security did not answer within {} s",
                TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout)?;
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_string(&mut stderr)?;
    }
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

#[cfg(test)]
pub mod fake {
    use std::{cell::RefCell, collections::HashMap};

    use super::*;

    #[derive(Default)]
    pub struct Fake {
        pub items: RefCell<HashMap<(String, String), String>>,
        pub fail_writes_to: RefCell<Option<String>>,
    }

    impl Secrets for Fake {
        fn read(&self, service: &str, account: &str) -> Result<Option<String>> {
            Ok(self
                .items
                .borrow()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }

        fn write(&self, service: &str, account: &str, secret: &str) -> Result<()> {
            if self.fail_writes_to.borrow().as_deref() == Some(service) {
                bail!("write to {service} failed");
            }
            self.items.borrow_mut().insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> Result<()> {
            self.items
                .borrow_mut()
                .remove(&(service.to_string(), account.to_string()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_values_are_quoted_for_security_i() {
        assert_eq!(
            quote("Claude Code-credentials"),
            "\"Claude Code-credentials\""
        );
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
