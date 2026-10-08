use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Kind {
    GitHub,
    Git,
    #[default]
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Repo {
        org: String,
        repo: String,
        kind: Kind,
        project: PathBuf,
        worktree: Option<String>,
    },
    // Directly in the base dir or an org folder, not inside a project.
    Loose(PathBuf),
    Elsewhere,
}

impl Place {
    // From the path alone: the org and repo are folder names until `identity` resolves them.
    pub fn of(base: &Path, cwd: &Path) -> Self {
        let Ok(rel) = cwd.strip_prefix(base) else {
            return Self::Elsewhere;
        };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        let repo = |org: &str, project: &str, worktree: Option<&str>| Self::Repo {
            org: org.to_string(),
            repo: project.to_string(),
            kind: Kind::Folder,
            project: base.join(org).join(project),
            worktree: worktree.map(String::from),
        };
        match parts.as_slice() {
            [org, project, ".claude", "worktrees", worktree, ..] => {
                repo(org, project, Some(worktree))
            }
            [org, project, ..] => repo(org, project, None),
            [] => Self::Loose(base.to_path_buf()),
            [org] => Self::Loose(base.join(org)),
        }
    }

    pub fn group(&self) -> String {
        match self {
            Self::Repo { org, repo, .. } => format!("{org} / {repo}"),
            Self::Loose(dir) => tilde(dir),
            Self::Elsewhere => "elsewhere".to_string(),
        }
    }

    pub fn worktree(&self) -> Option<&str> {
        match self {
            Self::Repo { worktree, .. } => worktree.as_deref(),
            _ => None,
        }
    }

    // The checkout the session works in: its worktree, or the project folder.
    pub fn root(&self) -> Option<PathBuf> {
        match self {
            Self::Repo {
                project,
                worktree: Some(name),
                ..
            } => Some(project.join(".claude/worktrees").join(name)),
            Self::Repo {
                project,
                worktree: None,
                ..
            } => Some(project.clone()),
            _ => None,
        }
    }
}

pub fn tilde(path: &Path) -> String {
    let path = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.into_owned(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Org(String),
    Repo {
        org: String,
        repo: String,
    },
    Loose(PathBuf),
    Elsewhere,
}

impl Scope {
    pub fn matches(&self, place: &Place) -> bool {
        match (self, place) {
            (Self::All, _) => true,
            (Self::Org(o), Place::Repo { org, .. }) => o == org,
            (Self::Repo { org: o, repo: r }, Place::Repo { org, repo, .. }) => {
                o == org && r == repo
            }
            (Self::Loose(d), Place::Loose(dir)) => d == dir,
            (Self::Elsewhere, Place::Elsewhere) => true,
            _ => false,
        }
    }

    pub fn for_places<'a>(places: impl IntoIterator<Item = &'a Place>) -> Vec<Self> {
        let mut repos: Vec<(String, String)> = Vec::new();
        let mut loose: Vec<PathBuf> = Vec::new();
        let mut elsewhere = false;
        for place in places {
            match place {
                Place::Repo { org, repo, .. } => {
                    if !repos.iter().any(|(o, r)| o == org && r == repo) {
                        repos.push((org.clone(), repo.clone()));
                    }
                }
                Place::Loose(dir) => {
                    if !loose.contains(dir) {
                        loose.push(dir.clone());
                    }
                }
                Place::Elsewhere => elsewhere = true,
            }
        }
        repos.sort();
        loose.sort();
        let mut scopes = vec![Self::All];
        let mut org = None;
        for (o, r) in repos {
            if org.as_ref() != Some(&o) {
                scopes.push(Self::Org(o.clone()));
                org = Some(o.clone());
            }
            scopes.push(Self::Repo { org: o, repo: r });
        }
        scopes.extend(loose.into_iter().map(Self::Loose));
        if elsewhere {
            scopes.push(Self::Elsewhere);
        }
        scopes
    }
}

#[cfg(test)]
pub fn repo(org: &str, repo: &str, worktree: Option<&str>) -> Place {
    Place::Repo {
        org: org.into(),
        repo: repo.into(),
        kind: Kind::Folder,
        project: PathBuf::from("/p").join(org).join(repo),
        worktree: worktree.map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(cwd: &str) -> Place {
        Place::of(Path::new("/p"), Path::new(cwd))
    }

    #[test]
    fn project_folders_and_their_subdirectories_are_repos() {
        assert_eq!(place("/p/acme/billing"), repo("acme", "billing", None));
        assert_eq!(
            place("/p/acme/billing/src/main"),
            repo("acme", "billing", None)
        );
    }

    #[test]
    fn worktrees_are_recognised_with_their_checkout_root() {
        let p = place("/p/globex/engine/.claude/worktrees/hover/sites");
        assert_eq!(p, repo("globex", "engine", Some("hover")));
        assert_eq!(
            p.root(),
            Some(PathBuf::from("/p/globex/engine/.claude/worktrees/hover"))
        );
    }

    #[test]
    fn base_and_org_folders_are_loose_and_anything_else_is_elsewhere() {
        assert_eq!(place("/p"), Place::Loose(PathBuf::from("/p")));
        assert_eq!(place("/p/acme"), Place::Loose(PathBuf::from("/p/acme")));
        assert_eq!(place("/tmp/elsewhere"), Place::Elsewhere);
        assert_eq!(Place::Elsewhere.group(), "elsewhere");
    }

    #[test]
    fn scopes_list_orgs_with_repos_then_loose_folders_then_elsewhere() {
        let places = [
            repo("globex", "engine", None),
            Place::Elsewhere,
            Place::Loose(PathBuf::from("/p")),
            repo("acme", "billing", None),
            repo("acme", "billing", Some("x")),
        ];
        assert_eq!(
            Scope::for_places(&places),
            [
                Scope::All,
                Scope::Org("acme".into()),
                Scope::Repo {
                    org: "acme".into(),
                    repo: "billing".into()
                },
                Scope::Org("globex".into()),
                Scope::Repo {
                    org: "globex".into(),
                    repo: "engine".into()
                },
                Scope::Loose(PathBuf::from("/p")),
                Scope::Elsewhere,
            ]
        );
    }

    #[test]
    fn org_scope_matches_all_its_repos_only() {
        let scope = Scope::Org("acme".into());
        assert!(scope.matches(&repo("acme", "billing", None)));
        assert!(!scope.matches(&repo("globex", "engine", None)));
        assert!(!scope.matches(&Place::Elsewhere));
    }
}
