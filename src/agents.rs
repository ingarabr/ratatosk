use std::{collections::HashMap, fs, path::PathBuf, process::Command};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Working,
    Blocked,
    Done,
    Unknown,
}

impl State {
    fn parse(state: Option<&str>) -> Self {
        match state {
            Some("working") => Self::Working,
            Some("blocked") => Self::Blocked,
            Some("done") => Self::Done,
            _ => Self::Unknown,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Working => "●",
            Self::Blocked => "◆",
            Self::Done => "○",
            Self::Unknown => "·",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub state: State,
    pub cwd: PathBuf,
    pub started_at_ms: Option<u64>,
    pub detail: Option<String>,
    pub prs: Vec<PrRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    fn from_href(href: &str) -> Option<Self> {
        let path = href.strip_prefix("https://github.com/")?;
        let mut parts = path.split('/');
        let (owner, name, kind, number) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        (kind == "pull").then_some(())?;
        Some(Self {
            repo: format!("{owner}/{name}"),
            number: number.parse().ok()?,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRow {
    id: String,
    cwd: PathBuf,
    name: Option<String>,
    state: Option<String>,
    session_id: Option<String>,
    started_at: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveSession {
    session_id: String,
    cwd: PathBuf,
}

pub fn load() -> Result<Vec<Session>> {
    let out = Command::new("claude")
        .args(["agents", "--json", "--all"])
        .output()
        .context("could not run `claude agents --json --all`")?;
    ensure!(
        out.status.success(),
        "`claude agents --json` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let rows: Vec<AgentRow> = serde_json::from_slice(&out.stdout)
        .context("unexpected output from `claude agents --json`")?;
    // `claude agents` reports where a session started; its session file has where it is now (e.g. after EnterWorktree).
    let live = live_cwds();
    Ok(rows
        .into_iter()
        .map(|row| {
            let job = job_state(&row.id);
            Session {
                cwd: row
                    .session_id
                    .as_deref()
                    .and_then(|id| live.get(id))
                    .cloned()
                    .unwrap_or(row.cwd),
                name: row.name.unwrap_or_else(|| row.id.clone()),
                detail: job
                    .as_ref()
                    .and_then(|j| j.detail.clone())
                    .filter(|d| !d.trim().is_empty()),
                prs: job
                    .as_ref()
                    .map(|j| {
                        j.children
                            .iter()
                            .filter(|c| c.kind == "pr")
                            .filter_map(|c| PrRef::from_href(&c.href))
                            .collect()
                    })
                    .unwrap_or_default(),
                id: row.id,
                state: State::parse(row.state.as_deref()),
                started_at_ms: row.started_at,
            }
        })
        .collect())
}

#[derive(Deserialize)]
struct JobState {
    detail: Option<String>,
    #[serde(default)]
    children: Vec<JobChild>,
}

#[derive(Deserialize)]
struct JobChild {
    href: String,
    kind: String,
}

// The agents view's own record of a session: its one-line status (`detail`) and the PRs it
// found in the transcript (`children`). The file is undocumented, so a read or parse failure
// just leaves both out.
fn job_state(id: &str) -> Option<JobState> {
    let home = std::env::var_os("HOME")?;
    let bytes = fs::read(
        PathBuf::from(home)
            .join(".claude/jobs")
            .join(id)
            .join("state.json"),
    )
    .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn stop(id: &str) -> Result<String> {
    run(&["stop", id])
}

pub fn remove(id: &str) -> Result<String> {
    run(&["rm", id])
}

fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("claude")
        .args(args)
        .output()
        .with_context(|| format!("could not run `claude {}`", args.join(" ")))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    ensure!(
        out.status.success(),
        "`claude {}` failed: {}",
        args.join(" "),
        text(&out.stderr)
    );
    Ok(text(&out.stdout))
}

fn live_cwds() -> HashMap<String, PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return HashMap::new();
    };
    let Ok(entries) = fs::read_dir(PathBuf::from(home).join(".claude/sessions")) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<LiveSession>(&bytes).ok())
        .map(|session| (session.session_id, session.cwd))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_links_are_read_from_github_pull_urls() {
        assert_eq!(
            PrRef::from_href("https://github.com/acme/billing/pull/1931"),
            Some(PrRef {
                repo: "acme/billing".into(),
                number: 1931
            })
        );
        assert_eq!(
            PrRef::from_href("https://github.com/acme/billing/issues/7"),
            None
        );
        assert_eq!(
            PrRef::from_href("https://gitlab.com/acme/billing/-/merge_requests/3"),
            None
        );
    }
}
