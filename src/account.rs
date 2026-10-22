use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::place::tilde;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountConfig {
    pub config_dir: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub orgs: Vec<String>,
}

pub const DEFAULT: usize = 0;

// A Claude Code login: one CLAUDE_CONFIG_DIR, with its own Keychain entry, daemon, session list,
// transcripts and folder trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    // None leaves CLAUDE_CONFIG_DIR as Ratatosk got it. Claude Code keys the Keychain entry by
    // the variable's literal value, so naming the default directory explicitly looks logged out.
    dir: Option<PathBuf>,
    state: PathBuf,
    global: PathBuf,
    pub email: Option<String>,
    orgs: Vec<String>,
}

impl Account {
    pub fn claude(&self) -> Command {
        let mut cmd = Command::new("claude");
        if let Some(dir) = &self.dir {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        cmd
    }

    // Repos of a pinned account's orgs never run on any other account.
    pub fn pinned(&self) -> bool {
        !self.orgs.is_empty()
    }

    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    // Holds folder trust, among other things.
    pub fn global_config(&self) -> &Path {
        &self.global
    }

    pub fn env_prefix(&self) -> String {
        self.dir
            .as_ref()
            .map(|dir| format!("CLAUDE_CONFIG_DIR={} ", tilde(dir)))
            .unwrap_or_default()
    }

    pub fn login_command(&self) -> String {
        let email = self
            .email
            .as_ref()
            .map(|e| format!(" --email {e}"))
            .unwrap_or_default();
        format!("{}claude auth login{email}", self.env_prefix())
    }

    pub fn check_login(&self) -> Result<()> {
        let out = self
            .claude()
            .args(["auth", "status", "--json"])
            .output()
            .context("could not run `claude auth status`")?;
        let status = serde_json::from_slice(&out.stdout)
            .context("unexpected output from `claude auth status --json`")?;
        self.verify(&status)
    }

    fn verify(&self, status: &AuthStatus) -> Result<()> {
        ensure!(
            status.logged_in,
            "the {} account is not logged in; run `{}` in a terminal",
            self.name,
            self.login_command()
        );
        if let Some(want) = &self.email {
            let got = status.email.as_deref().unwrap_or("an unknown email");
            ensure!(
                got.eq_ignore_ascii_case(want),
                "the {} account is logged in as {got}, not {want}; run `{}` in a terminal",
                self.name,
                self.login_command()
            );
        }
        Ok(())
    }

    // A transcript and its subagents' folder, found by session id since the project folder's
    // name is Claude Code's own encoding of the working directory.
    fn transcript(&self, session_id: &str) -> Option<(PathBuf, String)> {
        let file = format!("{session_id}.jsonl");
        fs::read_dir(self.state.join("projects"))
            .ok()?
            .flatten()
            .find(|entry| entry.path().join(&file).is_file())
            .map(|entry| {
                (
                    entry.path(),
                    entry.file_name().to_string_lossy().into_owned(),
                )
            })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    logged_in: bool,
    email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Accounts {
    list: Vec<Account>,
    aliases: HashMap<String, String>,
}

impl Default for Accounts {
    fn default() -> Self {
        Self::new(&BTreeMap::new(), HashMap::new(), None, Path::new("/")).unwrap()
    }
}

impl Accounts {
    // `inherited` is the CLAUDE_CONFIG_DIR Ratatosk runs with, if any.
    pub fn new(
        configs: &BTreeMap<String, AccountConfig>,
        aliases: HashMap<String, String>,
        inherited: Option<PathBuf>,
        home: &Path,
    ) -> Result<Self> {
        let default_state = inherited.clone().unwrap_or_else(|| home.join(".claude"));
        let default_global = match &inherited {
            Some(dir) => dir.join(".claude.json"),
            None => home.join(".claude.json"),
        };
        let mut list = vec![Account {
            name: "default".to_string(),
            dir: None,
            state: default_state.clone(),
            global: default_global.clone(),
            email: None,
            orgs: Vec::new(),
        }];
        let canonical = |org: &str| aliases.get(org).cloned().unwrap_or_else(|| org.to_string());
        let mut owners: HashMap<String, &str> = HashMap::new();
        for (name, config) in configs {
            for org in &config.orgs {
                if let Some(other) = owners.insert(canonical(org), name) {
                    bail!("org {org} is pinned to both the {other} and the {name} account");
                }
            }
            let dir: PathBuf = crate::config::expand(&config.config_dir, home)
                .components()
                .collect();
            let global = if dir == default_state {
                default_global.clone()
            } else {
                dir.join(".claude.json")
            };
            list.push(Account {
                name: name.clone(),
                dir: (dir != default_state).then(|| dir.clone()),
                state: dir,
                global,
                email: config.email.clone(),
                orgs: config.orgs.iter().map(|o| canonical(o)).collect(),
            });
        }
        Ok(Self { list, aliases })
    }

    pub fn get(&self, i: usize) -> &Account {
        &self.list[i]
    }

    // One per config dir, so each daemon's sessions are listed once.
    pub fn distinct(&self) -> Vec<usize> {
        let mut seen: Vec<&Path> = Vec::new();
        (0..self.list.len())
            .filter(|&i| {
                let state = self.list[i].state.as_path();
                let new = !seen.contains(&state);
                seen.push(state);
                new
            })
            .collect()
    }

    pub fn for_org(&self, org: Option<&str>) -> usize {
        let Some(org) = org else { return DEFAULT };
        let org = self.aliases.get(org).map_or(org, String::as_str);
        self.list
            .iter()
            .position(|a| a.orgs.iter().any(|o| o == org))
            .unwrap_or(DEFAULT)
    }

    // Whether a session running on `account` belongs on a pinned one instead.
    pub fn misplaced(&self, account: usize, org: Option<&str>) -> Option<usize> {
        let want = self.for_org(org);
        (self.list[want].pinned() && self.list[want].state != self.list[account].state)
            .then_some(want)
    }
}

// Copies a stopped session's transcript into another account, so `--resume` finds it there.
// The repo's auto-memory comes along when the target has none for that folder yet.
pub fn copy_transcript(from: &Account, to: &Account, session_id: &str) -> Result<()> {
    let (folder, name) = from
        .transcript(session_id)
        .with_context(|| format!("no transcript for {session_id} in {}", tilde(&from.state)))?;
    let target = to.state.join("projects").join(name);
    fs::create_dir_all(&target).with_context(|| format!("could not create {}", tilde(&target)))?;
    let file = format!("{session_id}.jsonl");
    fs::copy(folder.join(&file), target.join(&file))
        .with_context(|| format!("could not copy {file} to {}", tilde(&target)))?;
    for (dir, only_if_missing) in [(session_id, false), ("memory", true)] {
        let (src, dst) = (folder.join(dir), target.join(dir));
        if src.is_dir() && !(only_if_missing && dst.exists()) {
            copy_dir(&src, &dst)?;
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("could not create {}", tilde(to)))?;
    for entry in fs::read_dir(from)?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).with_context(|| format!("could not copy {}", tilde(&src)))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accounts(inherited: Option<&str>) -> Accounts {
        let configs = BTreeMap::from([(
            "acme".to_string(),
            AccountConfig {
                config_dir: "~/.claude-acme/".into(),
                email: Some("me@acme.example".into()),
                orgs: vec!["acme".into()],
            },
        )]);
        let aliases = HashMap::from([("acme-folder".to_string(), "acme".to_string())]);
        Accounts::new(
            &configs,
            aliases,
            inherited.map(PathBuf::from),
            Path::new("/h"),
        )
        .unwrap()
    }

    #[test]
    fn pinned_orgs_and_their_aliases_use_their_account() {
        let a = accounts(None);
        assert_eq!(a.get(a.for_org(Some("acme"))).name, "acme");
        assert_eq!(a.get(a.for_org(Some("acme-folder"))).name, "acme");
        assert_eq!(a.for_org(Some("globex")), DEFAULT);
        assert_eq!(a.for_org(None), DEFAULT);
    }

    #[test]
    fn config_dirs_are_normalised_and_the_default_keeps_the_inherited_one() {
        let a = accounts(None);
        let acme = a.get(1);
        assert_eq!(acme.env_prefix(), "CLAUDE_CONFIG_DIR=/h/.claude-acme ");
        assert_eq!(
            acme.global_config(),
            Path::new("/h/.claude-acme/.claude.json")
        );
        assert_eq!(a.get(DEFAULT).env_prefix(), "");
        assert_eq!(a.get(DEFAULT).global_config(), Path::new("/h/.claude.json"));
        let a = accounts(Some("/h/.claude-home"));
        assert_eq!(
            a.get(DEFAULT).global_config(),
            Path::new("/h/.claude-home/.claude.json")
        );
    }

    #[test]
    fn an_account_on_the_default_dir_leaves_the_variable_unset() {
        let configs = BTreeMap::from([(
            "acme".to_string(),
            AccountConfig {
                config_dir: "~/.claude".into(),
                email: None,
                orgs: vec!["acme".into()],
            },
        )]);
        let a = Accounts::new(&configs, HashMap::new(), None, Path::new("/h")).unwrap();
        assert_eq!(a.get(1).env_prefix(), "");
        assert_eq!(a.distinct(), [DEFAULT]);
        assert_eq!(a.misplaced(DEFAULT, Some("acme")), None);
    }

    #[test]
    fn only_sessions_of_pinned_orgs_on_another_account_are_misplaced() {
        let a = accounts(None);
        assert_eq!(a.misplaced(DEFAULT, Some("acme")), Some(1));
        assert_eq!(a.misplaced(1, Some("acme")), None);
        assert_eq!(a.misplaced(1, Some("globex")), None);
        assert_eq!(a.misplaced(DEFAULT, None), None);
    }

    #[test]
    fn an_org_pinned_twice_is_an_error() {
        let config = |dir: &str| AccountConfig {
            config_dir: dir.into(),
            email: None,
            orgs: vec!["acme".into()],
        };
        let configs = BTreeMap::from([
            ("one".to_string(), config("~/.one")),
            ("two".to_string(), config("~/.two")),
        ]);
        assert!(Accounts::new(&configs, HashMap::new(), None, Path::new("/h")).is_err());
    }

    #[test]
    fn login_must_match_the_pinned_email() {
        let a = accounts(None);
        let status = |logged_in, email: &str| AuthStatus {
            logged_in,
            email: Some(email.into()),
        };
        assert!(a.get(1).verify(&status(true, "Me@Acme.example")).is_ok());
        let wrong = a
            .get(1)
            .verify(&status(true, "me@home.example"))
            .unwrap_err();
        assert!(format!("{wrong}").contains("logged in as me@home.example"));
        let out = a.get(1).verify(&status(false, "")).unwrap_err();
        assert!(format!("{out}").contains(
            "CLAUDE_CONFIG_DIR=/h/.claude-acme claude auth login --email me@acme.example"
        ));
    }

    #[test]
    fn transcripts_move_with_their_subagents_and_memory_only_when_missing() {
        let root = std::env::temp_dir().join(format!("ratatosk-account-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let account = |name: &str| Account {
            name: name.into(),
            dir: Some(root.join(name)),
            state: root.join(name),
            global: root.join(name).join(".claude.json"),
            email: None,
            orgs: Vec::new(),
        };
        let (from, to) = (account("from"), account("to"));
        let project = root.join("from/projects/-p-acme-billing");
        fs::create_dir_all(project.join("s1/subagents")).unwrap();
        fs::create_dir_all(project.join("memory")).unwrap();
        fs::write(project.join("s1.jsonl"), "{}").unwrap();
        fs::write(project.join("s1/subagents/a.jsonl"), "{}").unwrap();
        fs::write(project.join("memory/MEMORY.md"), "from").unwrap();
        copy_transcript(&from, &to, "s1").unwrap();
        let moved = root.join("to/projects/-p-acme-billing");
        assert!(moved.join("s1.jsonl").is_file());
        assert!(moved.join("s1/subagents/a.jsonl").is_file());
        assert_eq!(
            fs::read_to_string(moved.join("memory/MEMORY.md")).unwrap(),
            "from"
        );

        fs::write(moved.join("memory/MEMORY.md"), "kept").unwrap();
        fs::write(project.join("s2.jsonl"), "{}").unwrap();
        copy_transcript(&from, &to, "s2").unwrap();
        assert_eq!(
            fs::read_to_string(moved.join("memory/MEMORY.md")).unwrap(),
            "kept"
        );
        assert!(copy_transcript(&from, &to, "missing").is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
