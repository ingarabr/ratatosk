use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use crate::agents::PrRef;

// The PRs a session was linked to on the account it was moved from, by session id. Claude Code
// keeps those links per account, so the account a session moves to starts without them.
pub fn path() -> Option<PathBuf> {
    crate::state::dir().map(|dir| dir.join("moved-prs.json"))
}

pub fn load(path: &Path) -> HashMap<String, Vec<PrRef>> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, carried: &HashMap<String, Vec<PrRef>>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_vec_pretty(carried)?)
}

pub fn merge(into: &mut Vec<PrRef>, from: &[PrRef]) {
    for pr in from {
        if !into.contains(pr) {
            into.push(pr.clone());
        }
    }
}
