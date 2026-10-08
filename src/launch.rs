use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};

use crate::{
    config::ManualModel,
    identity::{Resolver, clone_score},
    picker::{Item, Picker},
    place::Kind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub org: String,
    pub name: String,
    pub dir: PathBuf,
    pub kind: Kind,
    pub folder_org: String,
    pub folder: String,
}

impl Repo {
    pub fn label(&self) -> String {
        format!("{}/{}", self.org, self.name)
    }

    pub fn folder_label(&self) -> String {
        format!("{}/{}", self.folder_org, self.folder)
    }
}

// Every project folder under <base>/<org>/, named by its identity. Several clones of one repo
// become one entry: the clone whose folder is named most like the repo.
pub fn discover(base: &Path, resolver: &mut Resolver) -> Vec<Repo> {
    let mut by_identity: HashMap<(String, String), Repo> = HashMap::new();
    for org_dir in subdirs(base) {
        let folder_org = name_of(&org_dir);
        for dir in subdirs(&org_dir) {
            let folder = name_of(&dir);
            let id = resolver.identify(&dir, &folder_org, &folder);
            let candidate = Repo {
                org: id.org,
                name: id.repo,
                dir,
                kind: id.kind,
                folder_org: folder_org.clone(),
                folder,
            };
            let key = (candidate.org.clone(), candidate.name.clone());
            let better = by_identity.get(&key).is_none_or(|kept| {
                (
                    clone_score(&candidate.folder, &candidate.name),
                    std::cmp::Reverse(candidate.folder.len()),
                ) > (
                    clone_score(&kept.folder, &kept.name),
                    std::cmp::Reverse(kept.folder.len()),
                )
            });
            if better {
                by_identity.insert(key, candidate);
            }
        }
    }
    let mut repos: Vec<Repo> = by_identity.into_values().collect();
    repos.sort_by_key(Repo::label);
    repos
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && !name_of(path).starts_with('.'))
                .collect()
        })
        .unwrap_or_default()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Default,
    Haiku,
    Sonnet,
    Opus,
    Fable,
}

impl Model {
    const ALL: [Self; 5] = [
        Self::Default,
        Self::Haiku,
        Self::Sonnet,
        Self::Opus,
        Self::Fable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Haiku => "haiku",
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
            Self::Fable => "fable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Default,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    const ALL: [Self; 6] = [
        Self::Default,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Project,
    Model,
    Effort,
}

impl Field {
    pub fn next(self) -> Self {
        match self {
            Self::Project => Self::Model,
            Self::Model | Self::Effort => Self::Effort,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Effort => Self::Model,
            Self::Model | Self::Project => Self::Project,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Repo(usize),
    New { org: String, name: String },
}

impl Target {
    pub fn org<'a>(&'a self, repos: &'a [Repo]) -> &'a str {
        match self {
            Self::Repo(i) => &repos[*i].org,
            Self::New { org, .. } => org,
        }
    }

    pub fn label(&self, repos: &[Repo]) -> String {
        match self {
            Self::Repo(i) => repos[*i].label(),
            Self::New { org, name } => format!("{org}/{name}"),
        }
    }

    // The folder, which stays put when a repo's GitHub name changes; the recent list uses it.
    pub fn key(&self, repos: &[Repo]) -> String {
        match self {
            Self::Repo(i) => repos[*i].folder_label(),
            Self::New { org, name } => format!("{org}/{name}"),
        }
    }

    fn dir(&self, base: &Path, repos: &[Repo]) -> PathBuf {
        match self {
            Self::Repo(i) => repos[*i].dir.clone(),
            Self::New { org, name } => base.join(org).join(name),
        }
    }
}

pub struct Draft {
    pub prompt: String,
    pub target: Option<Target>,
    pub picker: Picker,
    pub model: Option<Model>,
    pub effort: Option<Effort>,
    pub field: Field,
    pub notice: Option<String>,
    rule: ManualModel,
    model_picked: bool,
    effort_picked: bool,
}

impl Draft {
    pub fn new(
        prompt: String,
        target: Option<Target>,
        recent: Vec<usize>,
        repos: &[Repo],
        rule: ManualModel,
    ) -> Self {
        let mut draft = Self {
            prompt,
            field: if target.is_some() {
                Field::Model
            } else {
                Field::Project
            },
            target,
            picker: Picker::new(recent, repos),
            model: None,
            effort: None,
            notice: None,
            model_picked: false,
            effort_picked: false,
            rule,
        };
        draft.apply_rule(repos);
        draft
    }

    pub fn manual(&self, repos: &[Repo]) -> bool {
        self.rule
            .applies(&self.prompt, self.target.as_ref().map(|t| t.org(repos)))
    }

    fn apply_rule(&mut self, repos: &[Repo]) {
        let preset = !self.manual(repos);
        if !self.model_picked {
            self.model = preset.then_some(Model::Default);
        }
        if !self.effort_picked {
            self.effort = preset.then_some(Effort::Default);
        }
    }

    pub fn pick_current(&mut self, repos: &[Repo]) {
        let target = match self.picker.current(repos) {
            Some(Item::Recent(i) | Item::Repo(i)) => Target::Repo(i),
            Some(Item::Create { org, name }) => Target::New { org, name },
            Some(Item::Org { .. }) => return self.picker.toggle_current(repos),
            _ => return,
        };
        self.target = Some(target);
        self.field = Field::Model;
        self.apply_rule(repos);
    }

    pub fn cycle(&mut self, step: isize) {
        match self.field {
            Field::Project => {}
            Field::Model => {
                let at = self
                    .model
                    .and_then(|m| Model::ALL.iter().position(|&x| x == m));
                self.model = Some(Model::ALL[step_index(Model::ALL.len(), at, step)]);
                self.model_picked = true;
            }
            Field::Effort => {
                let at = self
                    .effort
                    .and_then(|e| Effort::ALL.iter().position(|&x| x == e));
                self.effort = Some(Effort::ALL[step_index(Effort::ALL.len(), at, step)]);
                self.effort_picked = true;
            }
        }
    }

    pub fn launch(&self, base: &Path, repos: &[Repo]) -> Option<Launch> {
        let target = self.target.as_ref()?;
        Some(Launch {
            dir: target.dir(base, repos),
            create: matches!(target, Target::New { .. }),
            label: target.key(repos),
            prompt: self.prompt.clone(),
            model: self.model?,
            effort: self.effort?,
        })
    }
}

fn step_index(len: usize, at: Option<usize>, step: isize) -> usize {
    let len = len as isize;
    let from = at.map_or(if step > 0 { -1 } else { 0 }, |i| i as isize);
    (from + step).rem_euclid(len) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub dir: PathBuf,
    pub create: bool,
    pub label: String,
    pub prompt: String,
    pub model: Model,
    pub effort: Effort,
}

impl Launch {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["--bg".to_string()];
        if self.model != Model::Default {
            args.extend(["--model".to_string(), self.model.label().to_string()]);
        }
        if self.effort != Effort::Default {
            args.extend(["--effort".to_string(), self.effort.label().to_string()]);
        }
        args.push(prompt_arg(&self.prompt));
        args
    }

    pub fn start(&self) -> Result<String> {
        if self.create {
            fs::create_dir_all(&self.dir)
                .with_context(|| format!("could not create {}", self.dir.display()))?;
        }
        let out = Command::new("claude")
            .args(self.args())
            .current_dir(&self.dir)
            .output()
            .context("could not run `claude --bg`")?;
        ensure!(
            out.status.success(),
            "`claude --bg` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

// A prompt starting with '-' would be read as a flag; a leading space keeps it a prompt.
fn prompt_arg(prompt: &str) -> String {
    if prompt.starts_with('-') {
        format!(" {prompt}")
    } else {
        prompt.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acme() -> ManualModel {
        ManualModel {
            orgs: vec!["acme".into()],
            prompt_words: vec!["acme".into()],
            ..Default::default()
        }
    }

    fn repos() -> Vec<Repo> {
        ["acme/billing", "globex/engine", "ingarabr/ratatosk"]
            .iter()
            .map(|label| {
                let (org, name) = label.split_once('/').unwrap();
                Repo {
                    org: org.into(),
                    name: name.into(),
                    dir: PathBuf::from("/p").join(label),
                    kind: crate::place::Kind::Folder,
                    folder_org: org.into(),
                    folder: name.into(),
                }
            })
            .collect()
    }

    #[test]
    fn manual_project_or_prompt_leaves_model_and_effort_to_pick() {
        let repos = repos();
        let by_project = Draft::new(
            "fix refunds".into(),
            Some(Target::Repo(0)),
            Vec::new(),
            &repos,
            acme(),
        );
        assert_eq!((by_project.model, by_project.effort), (None, None));
        let by_prompt = Draft::new(
            "acme: fix refunds".into(),
            Some(Target::Repo(1)),
            Vec::new(),
            &repos,
            acme(),
        );
        assert_eq!((by_prompt.model, by_prompt.effort), (None, None));
        let new_in_acme = Draft::new(
            "spike".into(),
            Some(Target::New {
                org: "acme".into(),
                name: "spike".into(),
            }),
            Vec::new(),
            &repos,
            acme(),
        );
        assert_eq!(new_in_acme.model, None);
        let other = Draft::new(
            "fix the parser".into(),
            Some(Target::Repo(1)),
            Vec::new(),
            &repos,
            acme(),
        );
        assert_eq!(
            (other.model, other.effort),
            (Some(Model::Default), Some(Effort::Default))
        );
    }

    #[test]
    fn picking_a_manual_project_clears_presets_but_keeps_picks() {
        let repos = repos();
        let mut draft = Draft::new(
            "fix it".into(),
            Some(Target::Repo(1)),
            Vec::new(),
            &repos,
            acme(),
        );
        draft.cycle(1);
        draft.field = Field::Project;
        for c in "billing".chars() {
            draft.picker.type_char(&repos, c);
        }
        draft.pick_current(&repos);
        assert_eq!(draft.target, Some(Target::Repo(0)));
        assert_eq!(draft.model, Some(Model::Haiku));
        assert_eq!(draft.effort, None);
        assert_eq!(draft.field, Field::Model);
    }

    #[test]
    fn a_new_project_launches_in_a_folder_to_create() {
        let repos = repos();
        let draft = Draft::new(
            "start it".into(),
            Some(Target::New {
                org: "ingarabr".into(),
                name: "tool".into(),
            }),
            Vec::new(),
            &repos,
            acme(),
        );
        let launch = draft.launch(Path::new("/p"), &repos).unwrap();
        assert_eq!(launch.dir, PathBuf::from("/p/ingarabr/tool"));
        assert!(launch.create);
        assert_eq!(launch.label, "ingarabr/tool");
    }

    #[test]
    fn args_skip_defaults_and_protect_a_dash_prompt() {
        let launch = Launch {
            dir: PathBuf::from("/p"),
            create: false,
            label: "ingarabr/ratatosk".into(),
            prompt: "-v is broken".into(),
            model: Model::Sonnet,
            effort: Effort::Default,
        };
        assert_eq!(
            launch.args(),
            ["--bg", "--model", "sonnet", " -v is broken"]
        );
    }
}
