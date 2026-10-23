use std::{
    path::{Path, PathBuf},
    process::Command,
};

// The main checkout a worktree belongs to; None outside git or in the main checkout itself.
pub fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let root = common.parent()?;
    (common.file_name()? == ".git" && root != dir).then(|| root.to_path_buf())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_leads_to_its_main_checkout() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("ratatosk-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("billing");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@acme.example",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ],
        );
        let worktree = repo.join(".claude/worktrees/refunds");
        git(
            &repo,
            &["worktree", "add", "-q", worktree.to_str().unwrap()],
        );
        assert_eq!(main_checkout(&worktree), Some(repo.clone()));
        assert_eq!(main_checkout(&repo), None);
        assert_eq!(main_checkout(&root), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
