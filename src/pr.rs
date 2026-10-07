use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Unknown,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub state: PrState,
    pub title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    url: String,
    state: String,
    is_draft: bool,
    title: String,
    head_ref_name: String,
}

#[derive(Debug, Default, Clone)]
pub struct RepoPrs {
    pub repo: String,
    pub by_branch: HashMap<String, u64>,
    pub by_number: HashMap<u64, Pr>,
}

impl RepoPrs {
    fn add(&mut self, pr: GhPr) {
        if self.repo.is_empty() {
            self.repo = pr
                .url
                .split("/pull/")
                .next()
                .unwrap_or("")
                .trim_start_matches("https://github.com/")
                .to_string();
        }
        let state = match (pr.state.as_str(), pr.is_draft) {
            ("OPEN", true) => PrState::Draft,
            ("OPEN", false) => PrState::Open,
            ("MERGED", _) => PrState::Merged,
            _ => PrState::Closed,
        };
        self.by_branch.entry(pr.head_ref_name).or_insert(pr.number);
        self.by_number.insert(
            pr.number,
            Pr {
                number: pr.number,
                state,
                title: pr.title,
            },
        );
    }

    pub fn merge(&mut self, other: Self) {
        if self.repo.is_empty() {
            self.repo = other.repo;
        }
        for (branch, number) in other.by_branch {
            self.by_branch.entry(branch).or_insert(number);
        }
        self.by_number.extend(other.by_number);
    }
}

pub fn parse(json: &[u8]) -> Result<RepoPrs> {
    let prs: Vec<GhPr> =
        serde_json::from_slice(json).context("unexpected output from `gh pr list`")?;
    let mut repo = RepoPrs::default();
    for pr in prs {
        repo.add(pr);
    }
    Ok(repo)
}

const FIELDS: &str = "number,url,state,isDraft,title,headRefName";

enum Where<'a> {
    Dir(&'a Path),
    Repo(&'a str),
}

fn gh(place: &Where, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("gh");
    cmd.args(args);
    match place {
        Where::Dir(dir) => cmd.current_dir(dir),
        Where::Repo(repo) => cmd.args(["-R", repo]),
    };
    let out = cmd.output().context("could not run `gh`")?;
    ensure!(
        out.status.success(),
        "`gh {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

fn list(place: &Where) -> Result<RepoPrs> {
    parse(&gh(
        place,
        &[
            "pr", "list", "--state", "all", "--limit", "200", "--json", FIELDS,
        ],
    )?)
}

fn view(repo: &str, number: u64) -> Result<GhPr> {
    let json = gh(
        &Where::Repo(repo),
        &["pr", "view", &number.to_string(), "--json", FIELDS],
    )?;
    serde_json::from_slice(&json).context("unexpected output from `gh pr view`")
}

pub type Found = (Option<PathBuf>, Result<RepoPrs>);

// Looks up the repos sessions run in (for branch matching), then every repo a session links
// a PR in, then each linked PR too old to be in the list, one by one.
pub fn look_up(dirs: Vec<PathBuf>, linked: Vec<(String, u64)>) -> Receiver<Found> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut seen: HashMap<String, RepoPrs> = HashMap::new();
        for dir in dirs {
            let found = list(&Where::Dir(&dir));
            if let Ok(repo) = &found {
                seen.insert(repo.repo.to_lowercase(), repo.clone());
            }
            if tx.send((Some(dir), found)).is_err() {
                return;
            }
        }
        let mut by_repo: HashMap<String, Vec<u64>> = HashMap::new();
        for (repo, number) in linked {
            by_repo.entry(repo).or_default().push(number);
        }
        for (repo, numbers) in by_repo {
            let mut known = match seen.get(&repo.to_lowercase()) {
                Some(prs) => prs.clone(),
                None => match list(&Where::Repo(&repo)) {
                    Ok(prs) => prs,
                    Err(err) => {
                        let _ = tx.send((None, Err(err)));
                        continue;
                    }
                },
            };
            for number in numbers {
                if !known.by_number.contains_key(&number)
                    && let Ok(pr) = view(&repo, number)
                {
                    known.add(pr);
                }
            }
            if tx.send((None, Ok(known))).is_err() {
                return;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_drafts_and_the_newest_pr_per_branch() {
        let json = br#"[
            {"number": 12, "state": "OPEN", "isDraft": true, "title": "Spike", "url": "https://github.com/acme/billing/pull/0", "headRefName": "spike"},
            {"number": 11, "state": "MERGED", "isDraft": false, "title": "Refunds", "url": "https://github.com/acme/billing/pull/0", "headRefName": "refunds"},
            {"number": 9, "state": "CLOSED", "isDraft": false, "title": "Old refunds", "url": "https://github.com/acme/billing/pull/0", "headRefName": "refunds"},
            {"number": 8, "state": "OPEN", "isDraft": false, "title": "Parser", "url": "https://github.com/acme/billing/pull/0", "headRefName": "parser"}
        ]"#;
        let prs = parse(json).unwrap();
        assert_eq!(prs.repo, "acme/billing");
        assert_eq!(prs.by_number[&12].state, PrState::Draft);
        assert_eq!(prs.by_branch["refunds"], 11);
        assert_eq!(prs.by_number[&11].state, PrState::Merged);
        assert_eq!(prs.by_number[&9].state, PrState::Closed);
        assert_eq!(prs.by_number[&8].state, PrState::Open);
    }
}
