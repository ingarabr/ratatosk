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
    pub active_at_ms: Option<u64>,
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
                active_at_ms: job
                    .as_ref()
                    .and_then(|j| j.updated_at.as_deref())
                    .and_then(iso_ms)
                    .or(row.started_at),
            }
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobState {
    detail: Option<String>,
    updated_at: Option<String>,
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

// Milliseconds since the epoch for a UTC timestamp like "2026-10-07T12:46:46.011Z".
fn iso_ms(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, sec) = (t.next()??, t.next()??, t.next()??);
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + h) * 60 + min) * 60_000 + sec * 1000 + millis).ok()
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
    fn iso_timestamps_convert_to_epoch_millis() {
        assert_eq!(iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso_ms("2026-10-07T12:46:46.011Z"), Some(1_791_377_206_011));
        assert_eq!(iso_ms("2024-02-29T23:59:59Z"), Some(1_709_251_199_000));
        assert_eq!(iso_ms("not a date"), None);
    }

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
