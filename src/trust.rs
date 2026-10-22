use std::{fs, path::Path};

use serde::Deserialize;

use crate::account::Account;

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

// Whether Claude Code's trust prompt was accepted for exactly this folder; `claude --bg`
// refuses to start in one that wasn't. Each account keeps its own answers. None when the
// account's .claude.json can't be read.
pub fn is_trusted(account: &Account, dir: &Path) -> Option<bool> {
    let bytes = fs::read(account.global_config()).ok()?;
    Some(trusted_in(&bytes, dir))
}

fn trusted_in(config: &[u8], dir: &Path) -> bool {
    serde_json::from_slice::<ClaudeConfig>(config)
        .ok()
        .and_then(|c| {
            c.projects
                .get(dir.to_string_lossy().as_ref())
                .map(|p| p.trusted)
        })
        .unwrap_or(false)
}

pub fn is_trust_error(message: &str) -> bool {
    message.contains("not trusted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_with_an_accepted_prompt_are_trusted() {
        let config = br#"{
            "projects": {
                "/p": { "hasTrustDialogAccepted": true },
                "/p/acme/billing": { "hasTrustDialogAccepted": true },
                "/p/acme/admin": { "hasTrustDialogAccepted": false }
            },
            "otherSetting": 1
        }"#;
        assert!(trusted_in(config, Path::new("/p/acme/billing")));
        assert!(!trusted_in(config, Path::new("/p/acme/admin")));
        assert!(
            !trusted_in(config, Path::new("/p/acme/new")),
            "a trusted parent does not count"
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
