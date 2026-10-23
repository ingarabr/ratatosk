//! Edits text in `$VISUAL` or `$EDITOR`, run in Ratatosk's terminal through the handoff.

use std::{
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
};

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;

use crate::handoff::{self, Outcome};

/// Opens `text` in the user's editor and gives back what was saved, or `None` when the editor
/// was left with a detach key.
pub fn edit(terminal: &mut DefaultTerminal, text: &str, keyboard: bool) -> Result<Option<String>> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "ratatosk-{}-{}.md",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))?;
    let outcome = handoff::run(terminal, command(&path), || false, keyboard);
    let saved = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    match outcome? {
        Outcome::Detached => Ok(None),
        Outcome::Ended => Ok(Some(trim_final_newline(saved?))),
    }
}

/// The editor setting may carry arguments (`code --wait`), so a shell reads it.
fn command(path: &std::path::Path) -> Command {
    let editor = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string());
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path);
    cmd
}

// Editors end a saved file with a newline the text didn't have.
fn trim_final_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_final_newline_goes() {
        assert_eq!(trim_final_newline("a\nb\n".into()), "a\nb");
        assert_eq!(trim_final_newline("a\n\n".into()), "a\n");
        assert_eq!(trim_final_newline("a".into()), "a");
    }
}
