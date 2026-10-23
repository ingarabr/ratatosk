//! What a text field holds and how keys and pastes change it. `input::Input` draws one.
//!
//! A collapsed paste sits in the text as one private-use character, so the cursor, word motions
//! and deletion treat it as a single unit. It is replaced by what was pasted on the way out.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PASTE_BASE: u32 = 0xE000;
const PASTE_LAST: u32 = 0xF8FF;
const COLLAPSE_CHARS: usize = 800;
const COLLAPSE_LINES: usize = 3;
const TAB: &str = "    ";

#[derive(Debug, Clone)]
pub struct TextField {
    text: String,
    cursor: usize,
    multiline: bool,
    pastes: Vec<String>,
    killed: String,
}

impl Default for TextField {
    fn default() -> Self {
        Self::new("")
    }
}

/// A run of text on one drawn line; `paste` marks a collapsed paste's label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub paste: bool,
}

/// A spot in a field's layout: the width it was laid out in, and a row and column in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub width: usize,
    pub row: usize,
    pub col: usize,
}

struct Unit {
    at: usize,
    text: String,
    paste: bool,
}

pub struct Layout {
    pub lines: Vec<Vec<Piece>>,
    /// Row and column of the cursor, in cells.
    pub cursor: (usize, usize),
}

impl From<&str> for TextField {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for TextField {
    fn from(text: String) -> Self {
        Self::new(&text)
    }
}

impl TextField {
    /// A field that takes line breaks, and collapses long pastes.
    pub fn new(text: &str) -> Self {
        let mut field = Self {
            text: String::new(),
            cursor: 0,
            multiline: true,
            pastes: Vec::new(),
            killed: String::new(),
        };
        field.insert(&clean(text, true));
        field
    }

    /// A field that stays on one line: line breaks typed or pasted into it become spaces.
    pub fn one_line(text: &str) -> Self {
        let mut field = Self::new("");
        field.multiline = false;
        field.insert(&flatten(text));
        field
    }

    /// The text as typed, with each collapsed paste as its placeholder character.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// The text with every collapsed paste put back in full.
    pub fn expanded(&self) -> String {
        self.text
            .chars()
            .map(|c| match self.paste_index(c) {
                Some(i) => self.pastes[i].clone(),
                None => c.to_string(),
            })
            .collect()
    }

    /// The text with every collapsed paste shown as its label.
    pub fn shown(&self) -> String {
        self.shown_from(&self.text)
    }

    /// The shown text on either side of the cursor.
    pub fn shown_around_cursor(&self) -> (String, String) {
        let (before, after) = self.text.split_at(self.cursor);
        (self.shown_from(before), self.shown_from(after))
    }

    pub fn insert_char(&mut self, c: char) {
        if c != '\n' || self.multiline {
            self.insert(c.encode_utf8(&mut [0; 4]));
        }
    }

    /// Nothing in a paste acts as a key. In a one-line field line breaks become spaces. In a
    /// multi-line one a long paste collapses to a label, and pasting the same text again opens
    /// it back up in place.
    pub fn paste(&mut self, pasted: &str) {
        if !self.multiline {
            return self.insert(&flatten(pasted));
        }
        let pasted = clean(pasted, true);
        if let Some(c) = self.collapsed(&pasted) {
            let at = self.text.find(c).unwrap_or(self.cursor);
            self.text.replace_range(at..at + c.len_utf8(), "");
            self.cursor = at;
            self.insert(&pasted.replace('\t', TAB));
        } else if pasted.chars().count() > COLLAPSE_CHARS || pasted.lines().count() > COLLAPSE_LINES
        {
            match char::from_u32(PASTE_BASE + self.pastes.len() as u32)
                .filter(|&c| c as u32 <= PASTE_LAST)
            {
                Some(c) => {
                    self.pastes.push(pasted);
                    self.insert_char(c);
                }
                None => self.insert(&pasted.replace('\t', TAB)),
            }
        } else {
            self.insert(&pasted.replace('\t', TAB));
        }
    }

    /// Applies an editing key and says whether it was one.
    ///
    /// The keys are the shell's (readline's), minus the ones Ratatosk uses for itself: Ctrl+W
    /// opens the PR, Ctrl+B, Ctrl+D, Ctrl+F and Ctrl+P have their own jobs. Cmd+⌫ is sent as
    /// Ctrl+U by Ghostty, so both delete to the start of the line.
    pub fn apply(&mut self, key: KeyEvent) -> bool {
        let m = key.modifiers;
        let ctrl = m.contains(KeyModifiers::CONTROL);
        let alt = m.contains(KeyModifiers::ALT);
        let cmd = m.contains(KeyModifiers::SUPER);
        let word = m.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Backspace if cmd => self.kill_to(self.line_start_or_join()),
            KeyCode::Backspace if word => self.kill_to(self.word_back()),
            KeyCode::Backspace => self.delete_to(self.prev()),
            KeyCode::Delete if word => self.kill_to(self.word_forward()),
            KeyCode::Delete => self.delete_to(self.next()),
            KeyCode::Left if cmd => self.cursor = self.line_start(),
            KeyCode::Right if cmd => self.cursor = self.line_end(),
            KeyCode::Left if word => self.cursor = self.word_back(),
            KeyCode::Right if word => self.cursor = self.word_forward(),
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Char('u') if ctrl => self.kill_to(self.line_start_or_join()),
            KeyCode::Char('k') if ctrl => self.kill_to(self.line_end_or_join()),
            KeyCode::Char('y') if ctrl => self.insert(&self.killed.clone()),
            KeyCode::Char('b') if alt => self.cursor = self.word_back(),
            KeyCode::Char('f') if alt => self.cursor = self.word_forward(),
            KeyCode::Char('d') if alt => self.kill_to(self.word_forward()),
            KeyCode::Char(c) if !ctrl && !cmd => self.insert(&clean(&c.to_string(), false)),
            _ => return false,
        }
        true
    }

    /// Lays the text out in `width` cells, wrapping at spaces and breaking words longer than a
    /// line, and says where the cursor lands.
    pub fn layout(&self, width: usize) -> Layout {
        let lines = self.units(width);
        let cursor = lines
            .iter()
            .enumerate()
            .find_map(|(row, line)| {
                let mut col = 0;
                line.iter().find_map(|unit| {
                    let found = (unit.at == self.cursor).then_some((row, col));
                    col += cells(&unit.text);
                    found
                })
            })
            .unwrap_or_else(|| (lines.len() - 1, line_width(&lines[lines.len() - 1])));
        let lines = lines
            .into_iter()
            .map(|line| {
                let mut pieces: Vec<Piece> = Vec::new();
                for Unit { text, paste, .. } in line {
                    match pieces.last_mut() {
                        Some(last) if last.paste == paste && !paste => last.text.push_str(&text),
                        _ => pieces.push(Piece { text, paste }),
                    }
                }
                pieces.retain(|p| !p.text.is_empty());
                pieces
            })
            .collect();
        Layout { lines, cursor }
    }

    /// Moves the cursor to `spot`, and says which collapsed paste was there, if one was. A
    /// spot past the end of a line lands at its end; on a wrapped line, before the space it
    /// wrapped at.
    pub fn click(&mut self, spot: Spot) -> Option<usize> {
        let Spot { width, row, col } = spot;
        let lines = self.units(width);
        let line = lines.get(row).or(lines.last())?;
        let mut left = 0;
        for unit in line {
            let w = cells(&unit.text);
            if col < left + w.max(1) && !unit.text.is_empty() {
                let c = self.text[unit.at..].chars().next()?;
                let paste = self.paste_index(c);
                let after = col >= left + w.div_ceil(2);
                self.cursor = if paste.is_some() || after {
                    unit.at + c.len_utf8()
                } else {
                    unit.at
                };
                return paste;
            }
            left += w;
        }
        self.cursor = match line.last() {
            Some(unit) if unit.text.is_empty() || unit.text == " " => unit.at,
            _ => lines
                .get(row + 1)
                .and_then(|next| next.first())
                .map_or(self.text.len(), |unit| unit.at),
        };
        None
    }

    /// The collapsed paste right before the cursor, or else right after it.
    pub fn paste_at_cursor(&self) -> Option<usize> {
        let before = self.text[..self.cursor].chars().next_back();
        let after = self.text[self.cursor..].chars().next();
        [before, after]
            .into_iter()
            .flatten()
            .find_map(|c| self.paste_index(c))
    }

    /// What collapsed paste `i` holds.
    pub fn pasted(&self, i: usize) -> &str {
        &self.pastes[i]
    }

    /// Whether collapsed paste `i` is still in the text.
    pub fn has_paste(&self, i: usize) -> bool {
        char::from_u32(PASTE_BASE + i as u32).is_some_and(|c| self.text.contains(c))
    }

    /// Takes collapsed paste `i` out of the text.
    pub fn remove_paste(&mut self, i: usize) {
        self.splice_paste(i, "");
    }

    /// Puts what paste `i` holds into the text in place of its label.
    pub fn inline_paste(&mut self, i: usize) {
        self.splice_paste(i, &self.pastes[i].replace('\t', TAB));
    }

    /// Gives collapsed paste `i` new contents; with none left it goes from the text too.
    pub fn replace_paste(&mut self, i: usize, text: &str) {
        let text = clean(text, true);
        if text.trim().is_empty() {
            self.remove_paste(i);
        } else {
            self.pastes[i] = text;
        }
    }

    /// Replaces the text with `text`, which holds the pastes in full, as from `expanded`. Each
    /// paste still found whole in it is collapsed again. The cursor goes to the end.
    pub fn set_expanded(&mut self, text: &str) {
        let mut text = clean(text, self.multiline);
        if !self.multiline {
            text = flatten(&text);
        }
        let mut order: Vec<usize> = (0..self.pastes.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.pastes[i].len()));
        for i in order {
            let c = char::from_u32(PASTE_BASE + i as u32).expect("in the private-use range");
            if !self.pastes[i].is_empty() && text.contains(&self.pastes[i]) {
                text = text.replacen(&self.pastes[i], c.encode_utf8(&mut [0; 4]), 1);
            }
        }
        self.text = text;
        self.cursor = self.text.len();
    }

    fn splice_paste(&mut self, i: usize, with: &str) {
        let Some(c) = char::from_u32(PASTE_BASE + i as u32) else {
            return;
        };
        let Some(at) = self.text.find(c) else {
            return;
        };
        let end = at + c.len_utf8();
        self.text.replace_range(at..end, with);
        if self.cursor >= end {
            self.cursor = self.cursor - (end - at) + with.len();
        } else if self.cursor > at {
            self.cursor = at;
        }
    }

    // The text cut into drawn lines of units: one character, a space, a collapsed paste's
    // label, or an empty unit for a line break.
    fn units(&self, width: usize) -> Vec<Vec<Unit>> {
        let width = width.max(1);
        let mut lines: Vec<Vec<Unit>> = vec![Vec::new()];
        let mut used = 0;
        let mut word: Vec<Unit> = Vec::new();
        let flush = |lines: &mut Vec<Vec<Unit>>, used: &mut usize, word: &mut Vec<Unit>| {
            let word_width: usize = word.iter().map(|u| cells(&u.text)).sum();
            if *used > 0 && *used + word_width > width {
                lines.push(Vec::new());
                *used = 0;
            }
            for unit in word.drain(..) {
                let w = cells(&unit.text);
                if *used > 0 && *used + w > width {
                    lines.push(Vec::new());
                    *used = 0;
                }
                *used += w;
                lines.last_mut().unwrap().push(unit);
            }
        };
        for (at, c) in self.text.char_indices() {
            let unit = |text: String, paste| Unit { at, text, paste };
            if c == '\n' {
                flush(&mut lines, &mut used, &mut word);
                lines.last_mut().unwrap().push(unit(String::new(), false));
                lines.push(Vec::new());
                used = 0;
            } else if c == ' ' {
                flush(&mut lines, &mut used, &mut word);
                used += 1;
                lines.last_mut().unwrap().push(unit(" ".into(), false));
            } else {
                word.push(match self.paste_index(c) {
                    Some(i) => unit(self.label(i), true),
                    None => unit(c.to_string(), false),
                });
            }
        }
        flush(&mut lines, &mut used, &mut word);
        lines
    }

    fn shown_from(&self, text: &str) -> String {
        text.chars()
            .map(|c| match self.paste_index(c) {
                Some(i) => self.label(i),
                None => c.to_string(),
            })
            .collect()
    }

    fn paste_index(&self, c: char) -> Option<usize> {
        let i = (c as u32).checked_sub(PASTE_BASE)? as usize;
        (i < self.pastes.len()).then_some(i)
    }

    fn label(&self, i: usize) -> String {
        match self.pastes[i].lines().count() {
            n if n > 1 => format!("[Pasted text #{} +{n} lines]", i + 1),
            _ => format!("[Pasted text #{}]", i + 1),
        }
    }

    fn collapsed(&self, pasted: &str) -> Option<char> {
        let i = self.pastes.iter().position(|p| p == pasted)?;
        let c = char::from_u32(PASTE_BASE + i as u32)?;
        self.text.contains(c).then_some(c)
    }

    fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn delete_to(&mut self, to: usize) {
        let range = self.cursor.min(to)..self.cursor.max(to);
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
    }

    fn kill_to(&mut self, to: usize) {
        let range = self.cursor.min(to)..self.cursor.max(to);
        if !range.is_empty() {
            self.killed = self.text[range].to_string();
        }
        self.delete_to(to);
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }

    // At the edge of a line these take the line break, so pressing again joins the lines.
    fn line_start_or_join(&self) -> usize {
        match self.line_start() {
            start if start == self.cursor => self.prev(),
            start => start,
        }
    }

    fn line_end_or_join(&self) -> usize {
        match self.line_end() {
            end if end == self.cursor => self.next(),
            end => end,
        }
    }

    /// Back to the start of the previous word, like readline's backward-word: the separators
    /// after it are passed too, so from the end of `acme/bil` it stops after `acme/`. A
    /// collapsed paste is a word of its own.
    fn word_back(&self) -> usize {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(|c| !self.is_word(c) && !self.is_paste(c));
        match trimmed.chars().next_back() {
            Some(c) if self.is_paste(c) => trimmed.len() - c.len_utf8(),
            _ => trimmed.trim_end_matches(|c| self.is_word(c)).len(),
        }
    }

    fn word_forward(&self) -> usize {
        let after = &self.text[self.cursor..];
        let trimmed = after.trim_start_matches(|c| !self.is_word(c) && !self.is_paste(c));
        let rest = match trimmed.chars().next() {
            Some(c) if self.is_paste(c) => &trimmed[c.len_utf8()..],
            _ => trimmed.trim_start_matches(|c| self.is_word(c)),
        };
        self.text.len() - rest.len()
    }

    fn is_word(&self, c: char) -> bool {
        c.is_alphanumeric() && !self.is_paste(c)
    }

    fn is_paste(&self, c: char) -> bool {
        self.paste_index(c).is_some()
    }
}

/// Keeps line breaks (as `\n`) and tabs only when `lines` is set, and drops other control
/// characters and anything in the range that marks a collapsed paste.
fn clean(text: &str, lines: bool) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|&c| (lines && matches!(c, '\n' | '\t')) || !c.is_control())
        .filter(|&c| !(PASTE_BASE..=PASTE_LAST).contains(&(c as u32)))
        .collect()
}

fn flatten(text: &str) -> String {
    let text = clean(text, true);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_width(line: &[Unit]) -> usize {
    line.iter().map(|u| cells(&u.text)).sum()
}

fn cells(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `|` marks the cursor, in and out.
    fn field(text: &str) -> TextField {
        let at = text.find('|').expect("a cursor");
        let mut field = TextField::new(&text.replace('|', ""));
        field.cursor = at;
        field
    }

    fn marked(field: &TextField) -> String {
        let (before, after) = field.shown_around_cursor();
        format!("{before}|{after}")
    }

    fn edited(text: &str, key: KeyEvent) -> String {
        let mut field = field(text);
        assert!(field.apply(key), "{key:?}");
        marked(&field)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn ctrl(c: char) -> KeyEvent {
        with(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt(c: char) -> KeyEvent {
        with(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn typing_and_backspace_act_at_the_cursor() {
        assert_eq!(edited("ab|", key(KeyCode::Backspace)), "a|");
        assert_eq!(edited("a|c", key(KeyCode::Char('b'))), "ab|c");
        assert_eq!(edited("a|bc", key(KeyCode::Backspace)), "|bc");
        assert_eq!(edited("a|bc", key(KeyCode::Delete)), "a|c");
        assert_eq!(edited("|", key(KeyCode::Backspace)), "|");
        assert_eq!(edited("æ|ø", key(KeyCode::Backspace)), "|ø");
    }

    #[test]
    fn arrows_move_by_character_and_by_word() {
        assert_eq!(edited("ab|c", key(KeyCode::Left)), "a|bc");
        assert_eq!(edited("ab|c", key(KeyCode::Right)), "abc|");
        assert_eq!(edited("|abc", key(KeyCode::Left)), "|abc");
        assert_eq!(
            edited("fix the tests|", with(KeyCode::Left, KeyModifiers::ALT)),
            "fix the |tests"
        );
        assert_eq!(edited("fix the| tests", alt('b')), "fix |the tests");
        assert_eq!(
            edited(
                "fix| the tests",
                with(KeyCode::Right, KeyModifiers::CONTROL)
            ),
            "fix the| tests"
        );
        assert_eq!(edited("|fix the", alt('f')), "fix| the");
    }

    #[test]
    fn line_motions_stay_on_the_cursor_line() {
        assert_eq!(edited("one\ntw|o\nthree", ctrl('a')), "one\n|two\nthree");
        assert_eq!(edited("one\ntw|o\nthree", ctrl('e')), "one\ntwo|\nthree");
        assert_eq!(edited("one\ntw|o", key(KeyCode::Home)), "one\n|two");
        assert_eq!(edited("on|e\ntwo", key(KeyCode::End)), "one|\ntwo");
        assert_eq!(
            edited("one tw|o", with(KeyCode::Left, KeyModifiers::SUPER)),
            "|one two"
        );
    }

    #[test]
    fn option_backspace_and_ctrl_backspace_delete_the_word_before_the_cursor() {
        let word = |m| with(KeyCode::Backspace, m);
        assert_eq!(
            edited("fix the tests|", word(KeyModifiers::ALT)),
            "fix the |"
        );
        assert_eq!(edited("fix the  |", word(KeyModifiers::CONTROL)), "fix |");
        assert_eq!(edited("acme/bil|", word(KeyModifiers::ALT)), "acme/|");
        assert_eq!(edited("acme/|", word(KeyModifiers::ALT)), "|");
        assert_eq!(edited("one\ntwo|", word(KeyModifiers::ALT)), "one\n|");
        assert_eq!(
            edited("fix the| tests", word(KeyModifiers::ALT)),
            "fix | tests"
        );
        assert_eq!(edited("|", word(KeyModifiers::ALT)), "|");
        assert_eq!(edited("fix |the tests", alt('d')), "fix | tests");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_kill_to_the_line_edges_and_ctrl_y_yanks_it_back() {
        assert_eq!(edited("fix the| tests", ctrl('u')), "| tests");
        assert_eq!(
            edited(
                "fix the| tests",
                with(KeyCode::Backspace, KeyModifiers::SUPER)
            ),
            "| tests"
        );
        assert_eq!(edited("fix the| tests", ctrl('k')), "fix the|");
        assert_eq!(edited("one\ntwo\n|three", ctrl('u')), "one\ntwo|three");
        assert_eq!(edited("one|\ntwo", ctrl('k')), "one|two");

        let mut f = field("fix the| tests");
        f.apply(ctrl('u'));
        f.apply(ctrl('e'));
        f.apply(ctrl('y'));
        assert_eq!(marked(&f), " testsfix the|");
    }

    #[test]
    fn ctrl_w_and_unknown_keys_are_left_to_the_caller() {
        let mut f = field("text|");
        assert!(!f.apply(key(KeyCode::Enter)));
        assert!(!f.apply(ctrl('x')));
        assert!(!f.apply(ctrl('w')));
        assert!(!f.apply(key(KeyCode::Up)));
        assert_eq!(marked(&f), "text|");
    }

    #[test]
    fn a_short_paste_goes_in_as_text_without_acting_on_its_newlines() {
        let mut f = field("fix |now");
        f.paste("the\r\nparser\t");
        assert_eq!(marked(&f), "fix the\nparser    |now");
        assert_eq!(f.expanded(), "fix the\nparser    now");
    }

    #[test]
    fn a_long_paste_collapses_to_one_unit_and_expands_on_the_way_out() {
        let trace = "panic at main.rs:1\n  frame 1\n  frame 2\n  frame 3\n";
        let mut f = field("why |");
        f.paste(trace);
        f.insert(" again");
        assert_eq!(marked(&f), "why [Pasted text #1 +4 lines] again|");
        assert_eq!(f.expanded(), format!("why {trace} again"));

        f.apply(alt('b'));
        f.apply(alt('b'));
        assert_eq!(marked(&f), "why |[Pasted text #1 +4 lines] again");
        f.apply(key(KeyCode::Right));
        assert_eq!(marked(&f), "why [Pasted text #1 +4 lines]| again");
        f.apply(with(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(marked(&f), "why | again");
        f.apply(alt('f'));
        assert_eq!(marked(&f), "why  again|");
    }

    #[test]
    fn a_long_single_line_paste_collapses_too() {
        let mut f = field("|");
        f.paste(&"x".repeat(COLLAPSE_CHARS + 1));
        assert_eq!(f.shown(), "[Pasted text #1]");
    }

    #[test]
    fn pasting_a_collapsed_text_again_opens_it_up() {
        let trace = "a\nb\nc\nd";
        let mut f = field("see |");
        f.paste(trace);
        f.paste(trace);
        assert_eq!(marked(&f), "see a\nb\nc\nd|");
    }

    #[test]
    fn typed_or_pasted_placeholder_characters_are_dropped() {
        let mut f = field("|");
        f.apply(key(KeyCode::Char('\u{E000}')));
        f.paste("a\u{E001}b");
        assert_eq!(f.text(), "ab");
    }

    #[test]
    fn a_one_line_field_flattens_a_paste_and_takes_no_line_breaks() {
        let mut f = TextField::one_line("na\nme");
        f.paste(&format!(
            " credit\n notes\t\r\n{}",
            "x\n".repeat(COLLAPSE_LINES)
        ));
        f.insert_char('\n');
        assert_eq!(marked(&f), "na mecredit notes x x x|");
    }

    fn with_trace(before: &str) -> TextField {
        let mut f = field(before);
        f.paste("a\nb\nc\nd");
        f
    }

    #[test]
    fn the_paste_at_the_cursor_is_found_on_either_side() {
        let mut f = with_trace("see |");
        assert_eq!(f.paste_at_cursor(), Some(0));
        f.apply(key(KeyCode::Left));
        assert_eq!(f.paste_at_cursor(), Some(0));
        f.apply(key(KeyCode::Left));
        assert_eq!(f.paste_at_cursor(), None);
    }

    #[test]
    fn a_paste_can_be_removed_put_inline_or_given_new_contents() {
        let mut f = with_trace("see |");
        f.insert(" now");
        f.replace_paste(0, "x\ny\nz\nw\nv");
        assert_eq!(f.expanded(), "see x\ny\nz\nw\nv now");

        let mut inline = f.clone();
        inline.inline_paste(0);
        assert_eq!(marked(&inline), "see x\ny\nz\nw\nv now|");

        f.remove_paste(0);
        assert_eq!(marked(&f), "see  now|");
        assert!(!f.has_paste(0));

        let mut emptied = with_trace("see |");
        emptied.replace_paste(0, "  \n");
        assert_eq!(marked(&emptied), "see |");
    }

    #[test]
    fn text_back_from_the_editor_keeps_whole_pastes_collapsed() {
        let mut f = with_trace("see |");
        f.set_expanded("look at a\nb\nc\nd please\n");
        assert_eq!(marked(&f), "look at [Pasted text #1 +4 lines] please\n|");

        f.set_expanded("look at a\nB\nc\nd");
        assert_eq!(f.shown(), "look at a\nB\nc\nd");
    }

    #[test]
    fn a_click_moves_the_cursor_and_names_a_paste_it_lands_on() {
        let mut f = with_trace("see |");
        f.insert(" now");
        let spot = |row, col| Spot {
            width: 80,
            row,
            col,
        };
        assert_eq!(f.click(spot(0, 1)), None);
        assert_eq!(marked(&f), "s|ee [Pasted text #1 +4 lines] now");
        assert_eq!(f.click(spot(0, 2)), None);
        assert_eq!(marked(&f), "se|e [Pasted text #1 +4 lines] now");
        assert_eq!(f.click(spot(0, 10)), Some(0));
        assert_eq!(marked(&f), "see [Pasted text #1 +4 lines]| now");
        assert_eq!(f.click(spot(0, 70)), None);
        assert_eq!(marked(&f), "see [Pasted text #1 +4 lines] now|");

        let mut lines = field("one\ntwo|");
        lines.click(spot(0, 40));
        assert_eq!(marked(&lines), "one|\ntwo");
        lines.click(spot(5, 0));
        assert_eq!(marked(&lines), "one\n|two");

        let mut wrapped = field("fix the parser|");
        wrapped.click(Spot {
            width: 9,
            row: 0,
            col: 8,
        });
        assert_eq!(marked(&wrapped), "fix the| parser");
    }

    #[test]
    fn layout_wraps_at_spaces_and_follows_the_cursor() {
        let f = field("fix the pa|rser now");
        let layout = f.layout(9);
        let lines: Vec<String> = layout
            .lines
            .iter()
            .map(|l| l.iter().map(|p| p.text.as_str()).collect())
            .collect();
        assert_eq!(lines, ["fix the ", "parser ", "now"]);
        assert_eq!(layout.cursor, (1, 2));

        assert_eq!(field("one\n|").layout(10).cursor, (1, 0));
        assert_eq!(field("abcdefghijkl|").layout(5).cursor, (2, 2));
        assert_eq!(field("ab |").layout(5).cursor, (0, 3));
    }

    #[test]
    fn layout_marks_a_collapsed_paste() {
        let mut f = field("see |");
        f.paste("a\nb\nc\nd");
        let layout = f.layout(80);
        assert_eq!(
            layout.lines[0],
            [
                Piece {
                    text: "see ".into(),
                    paste: false
                },
                Piece {
                    text: "[Pasted text #1 +4 lines]".into(),
                    paste: true
                }
            ]
        );
        assert_eq!(layout.cursor, (0, 29));
    }
}
