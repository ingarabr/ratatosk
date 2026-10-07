use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Repo {
        org: String,
        repo: String,
        worktree: Option<String>,
    },
    Outside,
}

impl Place {
    pub fn of(base: &Path, cwd: &Path) -> Self {
        let Ok(rel) = cwd.strip_prefix(base) else {
            return Self::Outside;
        };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        match parts.as_slice() {
            [org, repo, ".claude", "worktrees", worktree, ..] => Self::Repo {
                org: org.to_string(),
                repo: repo.to_string(),
                worktree: Some(worktree.to_string()),
            },
            [org, repo, ..] => Self::Repo {
                org: org.to_string(),
                repo: repo.to_string(),
                worktree: None,
            },
            _ => Self::Outside,
        }
    }

    pub fn group(&self) -> String {
        match self {
            Self::Repo { org, repo, .. } => format!("{org} / {repo}"),
            Self::Outside => "not in a repo".to_string(),
        }
    }

    pub fn worktree(&self) -> Option<&str> {
        match self {
            Self::Repo { worktree, .. } => worktree.as_deref(),
            Self::Outside => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(cwd: &str) -> Place {
        Place::of(Path::new("/home/u/projects"), Path::new(cwd))
    }

    #[test]
    fn repo_root_and_subdirectories_belong_to_the_repo() {
        let expected = Place::Repo {
            org: "acme".into(),
            repo: "billing".into(),
            worktree: None,
        };
        assert_eq!(place("/home/u/projects/acme/billing"), expected);
        assert_eq!(place("/home/u/projects/acme/billing/src/main"), expected);
    }

    #[test]
    fn worktree_is_recognised() {
        assert_eq!(
            place("/home/u/projects/globex/engine/.claude/worktrees/dhost-api/sites"),
            Place::Repo {
                org: "globex".into(),
                repo: "engine".into(),
                worktree: Some("dhost-api".into()),
            }
        );
    }

    #[test]
    fn base_dir_org_dir_and_elsewhere_are_outside() {
        assert_eq!(place("/home/u/projects"), Place::Outside);
        assert_eq!(place("/home/u/projects/acme"), Place::Outside);
        assert_eq!(place("/tmp/elsewhere"), Place::Outside);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Org(String),
    Repo { org: String, repo: String },
    Outside,
}

impl Scope {
    pub fn matches(&self, place: &Place) -> bool {
        match (self, place) {
            (Self::All, _) => true,
            (Self::Org(o), Place::Repo { org, .. }) => o == org,
            (Self::Repo { org: o, repo: r }, Place::Repo { org, repo, .. }) => {
                o == org && r == repo
            }
            (Self::Outside, Place::Outside) => true,
            _ => false,
        }
    }

    pub fn for_places<'a>(places: impl IntoIterator<Item = &'a Place>) -> Vec<Self> {
        let mut repos: Vec<(String, String)> = Vec::new();
        let mut outside = false;
        for place in places {
            match place {
                Place::Repo { org, repo, .. } => {
                    if !repos.iter().any(|(o, r)| o == org && r == repo) {
                        repos.push((org.clone(), repo.clone()));
                    }
                }
                Place::Outside => outside = true,
            }
        }
        repos.sort();
        let mut scopes = vec![Self::All];
        let mut org = None;
        for (o, r) in repos {
            if org.as_ref() != Some(&o) {
                scopes.push(Self::Org(o.clone()));
                org = Some(o.clone());
            }
            scopes.push(Self::Repo { org: o, repo: r });
        }
        if outside {
            scopes.push(Self::Outside);
        }
        scopes
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    fn repo(org: &str, repo: &str) -> Place {
        Place::Repo {
            org: org.into(),
            repo: repo.into(),
            worktree: None,
        }
    }

    #[test]
    fn scopes_list_orgs_with_their_repos_then_outside() {
        let places = [
            repo("globex", "engine"),
            Place::Outside,
            repo("acme", "billing"),
            repo("acme", "billing"),
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
                Scope::Outside,
            ]
        );
    }

    #[test]
    fn org_scope_matches_all_its_repos_only() {
        let scope = Scope::Org("acme".into());
        assert!(scope.matches(&repo("acme", "billing")));
        assert!(!scope.matches(&repo("globex", "engine")));
        assert!(!scope.matches(&Place::Outside));
    }
}
