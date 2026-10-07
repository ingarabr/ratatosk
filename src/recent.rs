use std::{
    fs, io,
    path::{Path, PathBuf},
};

const KEEP: usize = 20;

pub fn path() -> Option<PathBuf> {
    crate::state::dir().map(|dir| dir.join("recent"))
}

pub fn load(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn remember(path: &Path, label: &str) -> io::Result<()> {
    let mut labels = load(path);
    labels.retain(|l| l != label);
    labels.insert(0, label.to_string());
    labels.truncate(KEEP);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, labels.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_without_duplicates() {
        let path = std::env::temp_dir()
            .join(format!("ratatosk-recent-{}", std::process::id()))
            .join("recent");
        remember(&path, "acme/billing").unwrap();
        remember(&path, "globex/engine").unwrap();
        remember(&path, "acme/billing").unwrap();
        assert_eq!(load(&path), ["acme/billing", "globex/engine"]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
