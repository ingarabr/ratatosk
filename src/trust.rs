use std::{fs, path::Path};

use serde::Deserialize;

use crate::{account::Account, git};

#[derive(Deserialize)]
struct ClaudeConfig {
    #[serde(default)]
    projects: std::collections::HashMap<String, ProjectConfig>,
}

#[derive(Deserialize)]
struct ProjectConfig {
    #[serde(default, rename = "hasTrustDialogAccepted")]
    trusted: bool,
}

// Whether Claude Code trusts this folder: its trust prompt was accepted there or in a folder
// above it. Claude Code asks nothing in a folder under a trusted one, so waiting for an answer
// filed for the folder itself would wait forever. A worktree is also trusted through its main
// checkout, where Claude Code files the answer. Each account keeps its own answers. None when
// the account's .claude.json can't be read.
pub fn is_trusted(account: &Account, dir: &Path) -> Option<bool> {
    let bytes = fs::read(account.global_config()).ok()?;
    Some(
        trusted_in(&bytes, dir)
            || git::main_checkout(dir).is_some_and(|root| trusted_in(&bytes, &root)),
    )
}

fn trusted_in(config: &[u8], dir: &Path) -> bool {
    let Ok(config) = serde_json::from_slice::<ClaudeConfig>(config) else {
        return false;
    };
    dir.ancestors().any(|dir| {
        config
            .projects
            .get(dir.to_string_lossy().as_ref())
            .is_some_and(|p| p.trusted)
    })
}

pub fn is_trust_error(message: &str) -> bool {
    message.contains("not trusted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_trusted_when_it_or_a_folder_above_it_was_accepted() {
        let config = br#"{
            "projects": {
                "/p": { "hasTrustDialogAccepted": true },
                "/q/acme/billing": { "hasTrustDialogAccepted": true },
                "/q/acme/admin": { "hasTrustDialogAccepted": false }
            },
            "otherSetting": 1
        }"#;
        assert!(trusted_in(config, Path::new("/q/acme/billing")));
        assert!(trusted_in(config, Path::new("/q/acme/billing/sub")));
        assert!(!trusted_in(config, Path::new("/q/acme/admin")));
        assert!(!trusted_in(config, Path::new("/q/acme/new")));
        assert!(
            trusted_in(config, Path::new("/p/ingarabr/new")),
            "Claude Code asks nothing under a trusted folder, even where it filed `false`"
        );
    }

    #[test]
    fn recognises_claudes_refusal() {
        assert!(is_trust_error(
            "`claude --bg` failed: Workspace not trusted. Run `claude` in /p/x"
        ));
        assert!(!is_trust_error("`claude --bg` failed: something else"));
    }
}
