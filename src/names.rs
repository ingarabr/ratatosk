use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

pub fn path() -> Option<PathBuf> {
    crate::state::dir().map(|dir| dir.join("names.json"))
}

pub fn load(path: &Path) -> HashMap<String, String> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, names: &HashMap<String, String>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_vec_pretty(names)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let path = std::env::temp_dir()
            .join(format!("ratatosk-names-{}", std::process::id()))
            .join("names.json");
        let names = HashMap::from([("a1".to_string(), "billing refunds".to_string())]);
        save(&path, &names).unwrap();
        assert_eq!(load(&path), names);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
