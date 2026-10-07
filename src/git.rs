use std::{path::Path, process::Command};

pub fn branch(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(if name.is_empty() {
        "detached HEAD".to_string()
    } else {
        name
    })
}
