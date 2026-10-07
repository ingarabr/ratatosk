use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    pub base_dir: Option<String>,
    pub manual_model: ManualModel,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ManualModel {
    pub orgs: Vec<String>,
    pub prompt_words: Vec<String>,
}

impl ManualModel {
    pub fn applies(&self, prompt: &str, org: Option<&str>) -> bool {
        let prompt = prompt.to_lowercase();
        org.is_some_and(|org| self.orgs.iter().any(|o| o == org))
            || self
                .prompt_words
                .iter()
                .any(|word| prompt.contains(&word.to_lowercase()))
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let Some(path) = path() else {
            return Ok(Self::default());
        };
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("invalid config in {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
        }
    }

    pub fn base_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = std::env::var_os("RATATOSK_BASE_DIR") {
            return Ok(dir.into());
        }
        let home = home()?;
        Ok(match &self.base_dir {
            Some(dir) => expand(dir, &home),
            None => home.join("projects"),
        })
    }
}

fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RATATOSK_CONFIG") {
        return Some(path.into());
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().ok().map(|home| home.join(".config")))?;
    Some(config.join("ratatosk/config.json"))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn expand(dir: &str, home: &Path) -> PathBuf {
    match dir.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if dir == "~" => home.to_path_buf(),
        None => PathBuf::from(dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_base_dir_and_manual_model_from_json() {
        let config: Config = serde_json::from_str(
            r#"{ "baseDir": "~/code", "manualModel": { "orgs": ["acme"], "promptWords": ["acme"] } }"#,
        )
        .unwrap();
        assert_eq!(config.base_dir.as_deref(), Some("~/code"));
        assert_eq!(config.manual_model.orgs, ["acme"]);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(serde_json::from_str::<Config>(r#"{ "baseDri": "~/code" }"#).is_err());
    }

    #[test]
    fn manual_model_matches_org_or_prompt_word() {
        let rule = ManualModel {
            orgs: vec!["acme".into()],
            prompt_words: vec!["Acme".into()],
        };
        assert!(rule.applies("fix it", Some("acme")));
        assert!(rule.applies("fix the ACME invoice", Some("other")));
        assert!(!rule.applies("fix it", Some("other")));
        assert!(!ManualModel::default().applies("acme", Some("acme")));
    }

    #[test]
    fn tilde_expands_to_home() {
        assert_eq!(
            expand("~/code", Path::new("/home/u")),
            PathBuf::from("/home/u/code")
        );
        assert_eq!(
            expand("/srv/code", Path::new("/home/u")),
            PathBuf::from("/srv/code")
        );
    }
}
