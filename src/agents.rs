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
        .map(|row| Session {
            cwd: row
                .session_id
                .as_deref()
                .and_then(|id| live.get(id))
                .cloned()
                .unwrap_or(row.cwd),
            name: row.name.unwrap_or_else(|| row.id.clone()),
            detail: detail(&row.id),
            id: row.id,
            state: State::parse(row.state.as_deref()),
            started_at_ms: row.started_at,
        })
        .collect())
}

#[derive(Deserialize)]
struct JobState {
    detail: Option<String>,
}

// The one-line status the agents view shows. It lives in the undocumented job state file,
// so it is optional: any read or parse failure leaves it out.
fn detail(id: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let bytes = fs::read(
        PathBuf::from(home)
            .join(".claude/jobs")
            .join(id)
            .join("state.json"),
    )
    .ok()?;
    serde_json::from_slice::<JobState>(&bytes)
        .ok()?
        .detail
        .filter(|d| !d.trim().is_empty())
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
