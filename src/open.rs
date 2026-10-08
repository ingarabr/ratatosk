use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opener {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub when: Vec<String>,
}

impl Opener {
    pub fn defaults() -> Vec<Self> {
        let (name, program) = if cfg!(target_os = "macos") {
            ("Finder", "open")
        } else {
            ("File manager", "xdg-open")
        };
        vec![Self {
            name: name.into(),
            command: vec![program.into(), "{dir}".into()],
            when: Vec::new(),
        }]
    }

    pub fn fits(&self, files: &[String]) -> bool {
        self.when.is_empty()
            || self
                .when
                .iter()
                .any(|pattern| files.iter().any(|file| glob(pattern, file)))
    }

    pub fn launch(&self, dir: &Path) -> Result<()> {
        let dir = dir.to_string_lossy();
        let args: Vec<String> = self
            .command
            .iter()
            .map(|arg| arg.replace("{dir}", &dir))
            .collect();
        let Some((program, rest)) = args.split_first() else {
            bail!("the opener \"{}\" has an empty command", self.name);
        };
        Command::new(program)
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("could not run {program} for \"{}\"", self.name))?;
        Ok(())
    }
}

pub fn root_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

pub fn fitting(openers: &[Opener], dir: &Path) -> Vec<usize> {
    let files = root_files(dir);
    (0..openers.len())
        .filter(|&i| openers[i].fits(&files))
        .collect()
}

// `*` matches any run of characters; everything else matches itself.
fn glob(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    for (n, part) in parts.iter().enumerate() {
        if n == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opener(name: &str, when: &[&str]) -> Opener {
        Opener {
            name: name.into(),
            command: vec!["true".into(), "{dir}".into()],
            when: when.iter().map(|w| w.to_string()).collect(),
        }
    }

    #[test]
    fn glob_matches_names_and_wildcards() {
        assert!(glob("Cargo.toml", "Cargo.toml"));
        assert!(!glob("Cargo.toml", "Cargo.lock"));
        assert!(glob("build.gradle*", "build.gradle.kts"));
        assert!(glob("*.iml", "billing.iml"));
        assert!(glob("bleep*.yaml", "bleep.yaml"));
        assert!(!glob("*.iml", "billing.xml"));
    }

    #[test]
    fn openers_fit_by_root_files_or_always() {
        let files = vec!["Cargo.toml".to_string(), "README.md".to_string()];
        assert!(opener("RustRover", &["Cargo.toml"]).fits(&files));
        assert!(!opener("IntelliJ", &["pom.xml", "bleep.yaml"]).fits(&files));
        assert!(opener("Finder", &[]).fits(&files));
    }

    #[test]
    fn fitting_reads_the_folder() {
        let dir = std::env::temp_dir().join(format!("ratatosk-open-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pom.xml"), "").unwrap();
        let openers = [
            opener("IntelliJ", &["pom.xml"]),
            opener("RustRover", &["Cargo.toml"]),
            opener("Finder", &[]),
        ];
        assert_eq!(fitting(&openers, &dir), [0, 2]);
        fs::remove_dir_all(dir).unwrap();
    }
}
