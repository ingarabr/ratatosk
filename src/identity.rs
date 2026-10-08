use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::place::Kind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub org: String,
    pub repo: String,
    pub kind: Kind,
}

// GitHub owner and repo from `origin`, aliased; otherwise the folder names, with the
// folder org aliased so it can join a GitHub org's group.
pub fn identify(
    project: &Path,
    folder_org: &str,
    folder_name: &str,
    aliases: &HashMap<String, String>,
) -> Identity {
    let alias = |org: &str| aliases.get(org).cloned().unwrap_or_else(|| org.to_string());
    if let Some((owner, repo)) = origin_url(project).as_deref().and_then(github_slug) {
        return Identity {
            org: alias(&owner),
            repo,
            kind: Kind::GitHub,
        };
    }
    let kind = if project.join(".git").exists() {
        Kind::Git
    } else {
        Kind::Folder
    };
    Identity {
        org: alias(folder_org),
        repo: folder_name.to_string(),
        kind,
    }
}

pub fn github_slug(url: &str) -> Option<(String, String)> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| (owner.to_string(), repo.to_string()))
}

fn origin_url(project: &Path) -> Option<String> {
    let dot_git = project.join(".git");
    if dot_git.is_dir() {
        return origin_from_config(&fs::read_to_string(dot_git.join("config")).ok()?);
    }
    if !dot_git.exists() {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn origin_from_config(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

// How well a clone's folder name matches its repo: an exact match wins, then the longest
// shared prefix, so `asp` beats `asp2` for the repo `asp`.
pub fn clone_score(folder_name: &str, repo: &str) -> (bool, usize) {
    let shared = folder_name
        .chars()
        .zip(repo.chars())
        .take_while(|(a, b)| a == b)
        .count();
    (folder_name == repo, shared)
}

#[derive(Default)]
pub struct Resolver {
    aliases: HashMap<String, String>,
    cache: HashMap<PathBuf, Identity>,
}

impl Resolver {
    pub fn new(aliases: HashMap<String, String>) -> Self {
        Self {
            aliases,
            cache: HashMap::new(),
        }
    }

    pub fn identify(&mut self, project: &Path, folder_org: &str, folder_name: &str) -> Identity {
        let aliases = &self.aliases;
        self.cache
            .entry(project.to_path_buf())
            .or_insert_with(|| identify(project, folder_org, folder_name, aliases))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls_in_every_form() {
        let want = Some(("acme".to_string(), "billing".to_string()));
        assert_eq!(github_slug("git@github.com:acme/billing.git"), want);
        assert_eq!(github_slug("https://github.com/acme/billing"), want);
        assert_eq!(github_slug("ssh://git@github.com/acme/billing.git"), want);
        assert_eq!(github_slug("git@gitlab.com:acme/billing.git"), None);
    }

    #[test]
    fn origin_wins_over_other_remotes() {
        let config = r#"
[core]
	bare = false
[remote "upstream"]
	url = git@github.com:someone/billing.git
[remote "origin"]
	url = git@github.com:acme/billing.git
	fetch = +refs/heads/*:refs/remotes/origin/*
"#;
        assert_eq!(
            origin_from_config(config).as_deref(),
            Some("git@github.com:acme/billing.git")
        );
    }

    #[test]
    fn folders_without_a_github_origin_use_aliased_folder_names() {
        let dir = std::env::temp_dir().join(format!("ratatosk-identity-{}", std::process::id()));
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(
            dir.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@gitlab.com:x/y.git\n",
        )
        .unwrap();
        let aliases = HashMap::from([("globex".to_string(), "globex-dev".to_string())]);
        assert_eq!(
            identify(&dir, "globex", "spike", &aliases),
            Identity {
                org: "globex-dev".into(),
                repo: "spike".into(),
                kind: Kind::Git
            }
        );
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(identify(&dir, "acme", "notes", &aliases).kind, Kind::Folder);
    }

    #[test]
    fn the_clone_named_like_the_repo_is_the_main_one() {
        assert!(clone_score("billing", "billing") > clone_score("billing2", "billing"));
        assert!(clone_score("engine-2", "engine") > clone_score("parser", "engine"));
    }
}
