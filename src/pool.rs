use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{keychain::Secrets, lockfile::DirLock};

// Where Claude Code keeps the default login, under the account `keychain::user()`. The item is a
// JSON object; only its `claudeAiOauth` key belongs to the account. The rest (MCP and gateway
// tokens) belongs to the machine and stays put across switches.
pub const LIVE: &str = "Claude Code-credentials";
const OAUTH: &str = "claudeAiOauth";
// One item per email, holding that account's `claudeAiOauth`.
const STORE: &str = "Ratatosk pool";
// Claude Code 2.1.308's lock settings: `stale: 60000` for the two credential-refresh locks,
// 15000 for the secure-storage write lock, proper-lockfile's default 10 s for the config lock.
const CREDENTIAL_STALE: Duration = Duration::from_secs(60);
const STORAGE_STALE: Duration = Duration::from_secs(15);
const CONFIG_STALE: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const BACK_OFF: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub email: String,
    #[serde(default)]
    pub organization: Option<String>,
    // The account's `oauthAccount` from ~/.claude.json, which holds no secrets.
    #[serde(default)]
    pub oauth_account: Value,
}

struct Leaving {
    oauth: Option<Value>,
    email: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    members: Vec<Member>,
}

fn held(locks: &[&DirLock]) -> bool {
    locks.iter().all(|lock| lock.still_held())
}

// The accounts the default login (~/.claude) switches between. A switch rewrites Claude Code's
// own credential, so every running session on the default login follows within Claude Code's
// 30 s Keychain cache. Pinned accounts have config dirs of their own and never join.
pub struct Pool {
    claude_dir: PathBuf,
    global: PathBuf,
    index: Option<PathBuf>,
    user: String,
    pinned: Vec<String>,
    lock_timeout: Duration,
    #[cfg(test)]
    fail_config_write: bool,
    pub members: Vec<Member>,
}

impl Pool {
    pub fn new(home: &Path, index: Option<PathBuf>, user: String, pinned: Vec<String>) -> Self {
        let members = index
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
            .map(|index| index.members)
            .unwrap_or_default();
        Self {
            claude_dir: home.join(".claude"),
            global: home.join(".claude.json"),
            index,
            user,
            pinned: pinned.iter().map(|e| e.to_lowercase()).collect(),
            lock_timeout: LOCK_TIMEOUT,
            #[cfg(test)]
            fail_config_write: false,
            members,
        }
    }

    pub fn index_path() -> Option<PathBuf> {
        crate::state::dir().map(|dir| dir.join("pool.json"))
    }

    // The default login's account, from ~/.claude.json.
    pub fn active(&self) -> Option<Member> {
        let config: Value = serde_json::from_slice(&fs::read(&self.global).ok()?).ok()?;
        member_of(config.get("oauthAccount")?)
    }

    pub fn is_pinned(&self, email: &str) -> bool {
        self.pinned.contains(&email.to_lowercase())
    }

    fn check_supported(&self) -> Result<()> {
        for var in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"] {
            ensure!(
                std::env::var_os(var).is_none(),
                "switching accounts needs the default ~/.claude login, but {var} is set"
            );
        }
        ensure!(
            !self.claude_dir.join(".config.json").exists(),
            "Claude Code keeps its config in ~/.claude/.config.json, which Ratatosk doesn't switch"
        );
        ensure!(
            !self.claude_dir.join(".credentials.json").exists(),
            "Claude Code keeps this login in ~/.claude/.credentials.json instead of the Keychain; Ratatosk only switches Keychain logins"
        );
        let link = fs::symlink_metadata(&self.global)
            .with_context(|| format!("could not read {}", self.global.display()))?;
        ensure!(
            !link.file_type().is_symlink(),
            "{} is a symlink; Ratatosk won't replace it",
            self.global.display()
        );
        Ok(())
    }

    // Claude Code's two refresh locks, in its order. While they are held no session can refresh,
    // so the refresh token can't rotate. Like Claude Code, a busy second lock releases the first
    // before trying again, so neither side waits while holding what the other needs.
    fn refresh_locks(&self, start: Instant) -> Result<(DirLock, DirLock)> {
        let primary = self.claude_dir.join(".oauth_refresh.lock");
        let real = fs::canonicalize(&self.claude_dir).unwrap_or_else(|_| self.claude_dir.clone());
        let mut legacy = real.into_os_string();
        legacy.push(".lock");
        let legacy = PathBuf::from(legacy);
        loop {
            let left = self.lock_timeout.saturating_sub(start.elapsed());
            let first = DirLock::acquire(&primary, CREDENTIAL_STALE, left)?;
            match DirLock::acquire(&legacy, CREDENTIAL_STALE, Duration::ZERO) {
                Ok(second) => return Ok((first, second)),
                Err(err) if start.elapsed() > self.lock_timeout => return Err(err),
                Err(_) => {
                    drop(first);
                    thread::sleep(BACK_OFF);
                }
            }
        }
    }

    // The locks around Claude Code's Keychain read-modify-writes and its config writes. Claude
    // Code gives up on them after a few seconds and loses what it meant to save (a rotated MCP
    // token, say), so they are only ever tried, never waited on, and held for a few `security`
    // calls at most.
    fn write_locks(&self, start: Instant) -> Result<(DirLock, DirLock)> {
        let storage = self.claude_dir.join(".storage-write.lock");
        let mut config = self.global.clone().into_os_string();
        config.push(".lock");
        let config = PathBuf::from(config);
        loop {
            let tried =
                DirLock::acquire(&storage, STORAGE_STALE, Duration::ZERO).and_then(|first| {
                    DirLock::acquire(&config, CONFIG_STALE, Duration::ZERO)
                        .map(|second| (first, second))
                });
            match tried {
                Ok(locks) => return Ok(locks),
                Err(err) if start.elapsed() > self.lock_timeout => return Err(err),
                Err(_) => thread::sleep(BACK_OFF),
            }
        }
    }

    fn store(&mut self, secrets: &dyn Secrets, oauth: &Value, account: &Value) -> Result<Member> {
        let member = member_of(account).context("the sign-in has no email")?;
        ensure!(
            !self.is_pinned(&member.email),
            "{} is pinned to an org in the config, so it can't join the pool",
            member.email
        );
        secrets.write(
            STORE,
            &member.email.to_lowercase(),
            &serde_json::to_string(oauth)?,
        )?;
        match self.position(&member.email) {
            Some(i) => self.members[i] = member.clone(),
            None => self.members.push(member.clone()),
        }
        self.save_index()?;
        Ok(member)
    }

    // Saves the current login into the pool. Claude Code rotates the refresh token as it uses
    // it, so the stored copy of the active account is only good until then; saving it again
    // keeps it current for when the login is changed by something other than Ratatosk.
    pub fn capture(&mut self, secrets: &dyn Secrets) -> Result<Member> {
        self.check_supported()?;
        let _locks = self.refresh_locks(Instant::now())?;
        let live = parse_object(
            &secrets
                .read(LIVE, &self.user)?
                .context("Claude Code isn't signed in")?,
        )?;
        let oauth = live
            .get(OAUTH)
            .context("Claude Code isn't signed in to claude.ai")?;
        let config = read_json(&self.global)?;
        let account = config
            .get("oauthAccount")
            .context("~/.claude.json has no oauthAccount")?;
        self.store(secrets, oauth, account)
    }

    // A capture that gives up at once when Claude Code holds its locks.
    pub fn sync(&mut self, secrets: &dyn Secrets) -> Result<Member> {
        let timeout = std::mem::replace(&mut self.lock_timeout, Duration::from_millis(500));
        let result = self.capture(secrets);
        self.lock_timeout = timeout;
        result
    }

    // Adds the login that `claude auth login` left in a throwaway config dir, then removes that
    // dir's Keychain item. The tokens stay valid: only a logout revokes them.
    pub fn import(&mut self, secrets: &dyn Secrets, config_dir: &Path) -> Result<Member> {
        let dir = config_dir.to_string_lossy();
        // Claude Code hashes the NFC form, which only equals the raw bytes for ASCII.
        ensure!(dir.is_ascii(), "signing in needs an ASCII path for {dir}");
        let service = keychain_service(&dir);
        let raw = secrets.read(&service, &self.user)?;
        let _ = secrets.delete(&service, &self.user);
        let live = parse_object(&raw.context("the sign-in didn't finish")?)?;
        let oauth = live.get(OAUTH).context("the sign-in wasn't to claude.ai")?;
        let config = read_json(&config_dir.join(".claude.json"))?;
        let account = config
            .get("oauthAccount")
            .context("the sign-in left no account details")?;
        self.store(secrets, oauth, account)
    }

    pub fn switch(&mut self, secrets: &dyn Secrets, email: &str) -> Result<()> {
        self.check_supported()?;
        let target = self
            .position(email)
            .map(|i| self.members[i].clone())
            .with_context(|| format!("{email} isn't in the pool"))?;
        ensure!(
            !self.is_pinned(email),
            "{email} is pinned to an org in the config, so it can't be the default login"
        );
        ensure!(
            target.oauth_account.is_object(),
            "no account details stored for {email}; sign in to it again"
        );
        let target_oauth: Value = serde_json::from_str(
            &secrets
                .read(STORE, &email.to_lowercase())?
                .with_context(|| format!("no stored sign-in for {email}; sign in to it again"))?,
        )
        .with_context(|| format!("the stored sign-in for {email} is unreadable"))?;

        let start = Instant::now();
        let (primary, legacy) = self.refresh_locks(start)?;
        let leaving = self.save_leaving(secrets, email)?;
        let Some(leaving) = leaving else {
            return Ok(());
        };

        let (storage, config_lock) = self.write_locks(start)?;
        let before = secrets.read(LIVE, &self.user)?;
        let mut live = match &before {
            Some(raw) => parse_object(raw)?,
            None => Map::new(),
        };
        let mut config = read_json(&self.global)?;
        ensure!(
            live.get(OAUTH) == leaving.oauth.as_ref()
                && config
                    .get("oauthAccount")
                    .and_then(member_of)
                    .map(|m| m.email)
                    == leaving.email,
            "Claude Code's sign-in changed while switching; nothing was changed"
        );
        ensure!(
            held(&[&primary, &legacy, &storage, &config_lock]),
            "lost a Claude Code lock while switching; nothing was changed"
        );
        let object = config
            .as_object_mut()
            .context("~/.claude.json isn't a JSON object")?;
        live.insert(OAUTH.to_string(), target_oauth);
        let after = serde_json::to_string(&Value::Object(live))?;
        let written = secrets.write(LIVE, &self.user, &after).and_then(|()| {
            ensure!(
                secrets.read(LIVE, &self.user)?.as_deref() == Some(after.as_str()),
                "the Keychain didn't keep the new sign-in"
            );
            Ok(())
        });
        if let Err(err) = written {
            return Err(err).context(self.restore(secrets, before.as_deref()));
        }
        object.insert("oauthAccount".to_string(), target.oauth_account);
        #[cfg(test)]
        if self.fail_config_write {
            return Err(anyhow::anyhow!("injected"))
                .context(self.restore(secrets, before.as_deref()));
        }
        if let Err(err) = write_json(&self.global, &config) {
            return Err(err).context(self.restore(secrets, before.as_deref()));
        }
        Ok(())
    }

    // Saves the login being left, which must happen under the refresh locks: the token read here
    // is then the one Claude Code holds until the switch. None when `email` is already active.
    fn save_leaving(&mut self, secrets: &dyn Secrets, email: &str) -> Result<Option<Leaving>> {
        let live = match secrets.read(LIVE, &self.user)? {
            Some(raw) => parse_object(&raw)?,
            None => Map::new(),
        };
        let config = read_json(&self.global)?;
        let oauth = live.get(OAUTH).cloned();
        let account = config.get("oauthAccount").cloned();
        match (&oauth, &account) {
            (Some(oauth), Some(account)) => {
                if member_of(account).is_some_and(|m| same(&m.email, email)) {
                    return Ok(None);
                }
                self.store(secrets, oauth, account)
                    .context("could not save the current sign-in, so nothing was switched")?;
            }
            (Some(_), None) => bail!(
                "Claude Code is signed in, but ~/.claude.json doesn't say as whom; switching would lose that sign-in"
            ),
            (None, _) => {}
        }
        Ok(Some(Leaving {
            oauth,
            email: account.as_ref().and_then(member_of).map(|m| m.email),
        }))
    }

    fn restore(&self, secrets: &dyn Secrets, before: Option<&str>) -> &'static str {
        let undone = match before {
            Some(raw) => secrets.write(LIVE, &self.user, raw),
            None => secrets.delete(LIVE, &self.user),
        };
        match undone {
            Ok(()) => "the switch failed; the previous sign-in is back in place",
            Err(_) => {
                "the switch failed, and putting the previous sign-in back failed too; run `claude auth login`"
            }
        }
    }

    pub fn forget(&mut self, secrets: &dyn Secrets, email: &str) -> Result<()> {
        secrets.delete(STORE, &email.to_lowercase())?;
        self.members.retain(|m| !same(&m.email, email));
        self.save_index()
    }

    fn position(&self, email: &str) -> Option<usize> {
        self.members.iter().position(|m| same(&m.email, email))
    }

    fn save_index(&self) -> Result<()> {
        let Some(path) = &self.index else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let index = Index {
            members: self.members.clone(),
        };
        replace(path, serde_json::to_string_pretty(&index)?.as_bytes())
    }
}

// The Keychain service Claude Code uses for a CLAUDE_CONFIG_DIR: the first eight hex digits of
// the SHA-256 of the variable's exact value.
pub fn keychain_service(config_dir: &str) -> String {
    let digest = Sha256::digest(config_dir.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{LIVE}-{hex}")
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn member_of(account: &Value) -> Option<Member> {
    Some(Member {
        email: account.get("emailAddress")?.as_str()?.to_string(),
        organization: account
            .get("organizationName")
            .and_then(Value::as_str)
            .map(String::from),
        oauth_account: account.clone(),
    })
}

fn parse_object(raw: &str) -> Result<Map<String, Value>> {
    match serde_json::from_str(raw) {
        Ok(Value::Object(map)) => Ok(map),
        _ => bail!("Claude Code's Keychain item isn't in the format Ratatosk knows"),
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} isn't valid JSON", path.display()))
}

// The same shape Claude Code writes: two-space indent, keys in their order, no final newline.
fn write_json(path: &Path, value: &Value) -> Result<()> {
    replace(path, serde_json::to_string_pretty(value)?.as_bytes())
}

// Readers see the old file or the new one, never a partial write.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}.{nanos:08x}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        if let Ok(meta) = fs::metadata(path) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::fake::Fake;

    const LOCKS: [&str; 4] = [
        ".claude/.oauth_refresh.lock",
        ".claude.lock",
        ".claude/.storage-write.lock",
        ".claude.json.lock",
    ];

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ratatosk-pool-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join(".claude")).unwrap();
            fs::create_dir_all(dir.join("state")).unwrap();
            Self(fs::canonicalize(dir).unwrap())
        }

        fn signed_in(&self, secrets: &Fake, email: &str, token: &str) {
            secrets.write(LIVE, "me", &blob(token, "mcp-1")).unwrap();
            let config = format!(
                "{{\n  \"numStartups\": 3,\n  \"oauthAccount\": {{\n    \"emailAddress\": \"{email}\",\n    \"organizationName\": \"{email}'s Organization\"\n  }},\n  \"projects\": {{}}\n}}"
            );
            fs::write(self.0.join(".claude.json"), config).unwrap();
        }

        fn pool(&self, pinned: &[&str]) -> Pool {
            let mut pool = Pool::new(
                &self.0,
                Some(self.0.join("state/pool.json")),
                "me".into(),
                pinned.iter().map(|s| s.to_string()).collect(),
            );
            pool.lock_timeout = Duration::from_millis(300);
            pool
        }

        fn config(&self) -> String {
            fs::read_to_string(self.0.join(".claude.json")).unwrap()
        }

        fn two_accounts(&self, secrets: &Fake) -> Pool {
            self.signed_in(secrets, "a@acme.example", "token-a");
            let mut pool = self.pool(&[]);
            pool.capture(secrets).unwrap();
            self.signed_in(secrets, "b@globex.example", "token-b");
            pool.capture(secrets).unwrap();
            pool
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn blob(token: &str, mcp: &str) -> String {
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{token}"}},"mcpOAuth":{{"server":"{mcp}"}}}}"#
        )
    }

    fn live(secrets: &Fake) -> Value {
        serde_json::from_str(&secrets.read(LIVE, "me").unwrap().unwrap()).unwrap()
    }

    fn token(secrets: &Fake) -> String {
        live(secrets)[OAUTH]["accessToken"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn switching_saves_the_current_login_and_swaps_only_the_account() {
        let home = Home::new("switch");
        let secrets = Fake::default();
        let mut pool = home.two_accounts(&secrets);
        assert_eq!(pool.members.len(), 2);

        secrets
            .write(LIVE, "me", &blob("token-b2", "mcp-2"))
            .unwrap();
        pool.switch(&secrets, "A@acme.example").unwrap();
        assert_eq!(token(&secrets), "token-a");
        assert_eq!(
            live(&secrets)["mcpOAuth"]["server"],
            "mcp-2",
            "machine tokens stay"
        );
        assert_eq!(pool.active().unwrap().email, "a@acme.example");
        let config = home.config();
        assert!(config.starts_with("{\n  \"numStartups\": 3,\n  \"oauthAccount\""));
        assert!(
            config.ends_with("\"projects\": {}\n}"),
            "keys keep their order and format"
        );

        pool.switch(&secrets, "b@globex.example").unwrap();
        assert_eq!(
            token(&secrets),
            "token-b2",
            "the rotated token was saved on the way out"
        );
        for lock in LOCKS {
            assert!(!home.0.join(lock).exists(), "{lock} was released");
        }
        assert_eq!(home.pool(&[]).members, pool.members);
    }

    #[test]
    fn a_failed_config_write_puts_the_previous_login_back() {
        let home = Home::new("rollback");
        let secrets = Fake::default();
        let mut pool = home.two_accounts(&secrets);
        pool.fail_config_write = true;
        let err = format!("{:#}", pool.switch(&secrets, "a@acme.example").unwrap_err());
        assert!(err.contains("previous sign-in is back"), "{err}");
        assert_eq!(token(&secrets), "token-b");
        assert!(home.config().contains("b@globex.example"));
    }

    #[test]
    fn a_failed_keychain_write_changes_nothing() {
        let home = Home::new("keychain");
        let secrets = Fake::default();
        let mut pool = home.two_accounts(&secrets);
        *secrets.fail_writes_to.borrow_mut() = Some(LIVE.to_string());
        assert!(pool.switch(&secrets, "a@acme.example").is_err());
        assert_eq!(token(&secrets), "token-b");
        assert!(home.config().contains("b@globex.example"));
    }

    #[test]
    fn every_claude_lock_is_respected() {
        for lock in LOCKS {
            let home = Home::new("held");
            let secrets = Fake::default();
            let mut pool = home.two_accounts(&secrets);
            let held =
                DirLock::acquire(&home.0.join(lock), CREDENTIAL_STALE, Duration::ZERO).unwrap();
            let err = pool.switch(&secrets, "a@acme.example").unwrap_err();
            assert!(
                format!("{err:#}").contains("held by another process"),
                "{lock}"
            );
            assert_eq!(token(&secrets), "token-b");
            for other in LOCKS {
                assert_eq!(
                    home.0.join(other).exists(),
                    other == lock,
                    "{other} while {lock} is held"
                );
            }
            drop(held);
            pool.switch(&secrets, "a@acme.example").unwrap();
            assert_eq!(token(&secrets), "token-a");
        }
    }

    #[test]
    fn pinned_accounts_never_join_the_pool() {
        let home = Home::new("pinned");
        let secrets = Fake::default();
        home.signed_in(&secrets, "me@acme.example", "token");
        let mut pool = home.pool(&["Me@Acme.example"]);
        assert!(pool.capture(&secrets).is_err());
        assert!(pool.members.is_empty());
    }

    #[test]
    fn a_login_without_an_identity_is_never_overwritten() {
        let home = Home::new("anonymous");
        let secrets = Fake::default();
        let mut pool = home.two_accounts(&secrets);
        fs::write(home.0.join(".claude.json"), "{}").unwrap();
        assert!(pool.switch(&secrets, "a@acme.example").is_err());
        assert_eq!(token(&secrets), "token-b");
    }

    #[test]
    fn a_sign_in_elsewhere_is_imported_without_touching_the_live_login() {
        let home = Home::new("import");
        let secrets = Fake::default();
        let mut pool = home.two_accounts(&secrets);
        let dir = home.0.join("state/sign-in");
        fs::create_dir_all(&dir).unwrap();
        let service = keychain_service(&dir.to_string_lossy());
        secrets
            .write(&service, "me", &blob("token-c", "mcp-c"))
            .unwrap();
        fs::write(
            dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"c@initech.example"}}"#,
        )
        .unwrap();
        let member = pool.import(&secrets, &dir).unwrap();
        assert_eq!(member.email, "c@initech.example");
        assert_eq!(token(&secrets), "token-b");
        assert!(
            secrets.read(&service, "me").unwrap().is_none(),
            "the throwaway item is gone"
        );
        pool.switch(&secrets, "c@initech.example").unwrap();
        assert_eq!(token(&secrets), "token-c");
        assert_eq!(live(&secrets)["mcpOAuth"]["server"], "mcp-1");
    }

    #[test]
    fn config_dir_services_match_claude_code() {
        assert_eq!(
            keychain_service("/home/u/.claude-acme"),
            "Claude Code-credentials-9dd91b85"
        );
    }
}
