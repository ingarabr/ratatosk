//! Line editing shared by every text field: the prompt, the rename box and the picker filter.
//! None of them has a cursor, so editing acts on the end of the buffer.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Applies an editing key to `buffer` and says whether it was one.
///
/// Option+⌫ (or Ctrl+⌫) deletes the last word, Cmd+⌫ and Ctrl+U clear the field. Ghostty sends
/// Cmd+⌫ as Ctrl+U by default, so both spellings land here. Ctrl+W is kept for opening the PR.
pub fn apply(buffer: &mut String, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let word = KeyModifiers::ALT | KeyModifiers::CONTROL;
    match key.code {
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => buffer.clear(),
        KeyCode::Backspace if key.modifiers.intersects(word) => delete_word(buffer),
        KeyCode::Backspace => {
            buffer.pop();
        }
        KeyCode::Char('u') if ctrl => buffer.clear(),
        KeyCode::Char(c) if !ctrl => buffer.push(c),
        _ => return false,
    }
    true
}

/// Deletes back to the start of the last word, like readline's backward-kill-word: the
/// separators after it go too, so `acme/` becomes empty and `acme/bil` becomes `acme/`.
fn delete_word(buffer: &mut String) {
    let trimmed = buffer.trim_end_matches(|c: char| !c.is_alphanumeric());
    let end = trimmed.trim_end_matches(char::is_alphanumeric).len();
    buffer.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(text: &str, key: KeyEvent) -> String {
        let mut buffer = text.to_string();
        assert!(apply(&mut buffer, key));
        buffer
    }

    fn backspace(modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Backspace, modifiers)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn backspace_removes_one_char_and_typing_appends() {
        assert_eq!(edited("ab", backspace(KeyModifiers::NONE)), "a");
        assert_eq!(edited("a", KeyEvent::from(KeyCode::Char('b'))), "ab");
    }

    #[test]
    fn option_backspace_and_ctrl_backspace_delete_the_last_word() {
        assert_eq!(
            edited("fix the tests", backspace(KeyModifiers::ALT)),
            "fix the "
        );
        assert_eq!(
            edited("fix the  ", backspace(KeyModifiers::CONTROL)),
            "fix "
        );
        assert_eq!(edited("acme/bil", backspace(KeyModifiers::ALT)), "acme/");
        assert_eq!(edited("acme/", backspace(KeyModifiers::ALT)), "");
        assert_eq!(edited("one\ntwo", backspace(KeyModifiers::ALT)), "one\n");
        assert_eq!(edited("", backspace(KeyModifiers::ALT)), "");
    }

    #[test]
    fn cmd_backspace_and_ctrl_u_clear_the_field() {
        assert_eq!(edited("fix the\ntests", backspace(KeyModifiers::SUPER)), "");
        assert_eq!(edited("fix the tests", ctrl('u')), "");
    }

    #[test]
    fn other_keys_are_left_alone() {
        let mut buffer = "text".to_string();
        assert!(!apply(&mut buffer, KeyEvent::from(KeyCode::Enter)));
        assert!(!apply(&mut buffer, ctrl('x')));
        assert!(!apply(&mut buffer, ctrl('w')));
        assert_eq!(buffer, "text");
    }
}
