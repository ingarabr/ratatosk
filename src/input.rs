//! Draws a `TextField`. Every text field goes through `Input`, so they all wrap, scroll, show
//! collapsed pastes and place the cursor the same way.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::edit::TextField;

const CURSOR_BAR: &str = "▏";

pub struct Input<'a> {
    field: &'a TextField,
    prefix: Vec<Span<'a>>,
    placeholder: &'a str,
    max_lines: usize,
    style: Style,
}

impl<'a> Input<'a> {
    pub fn new(field: &'a TextField) -> Self {
        Self {
            field,
            prefix: Vec::new(),
            placeholder: "",
            max_lines: usize::MAX,
            style: Style::new(),
        }
    }

    /// Drawn before the first line; the lines after it are indented to match.
    pub fn prefix(mut self, prefix: impl Into<Line<'a>>) -> Self {
        self.prefix = prefix.into().spans;
        self
    }

    /// Shown dimmed while the field is empty.
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// The most lines shown; past it the text scrolls to keep the cursor in view.
    pub fn max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// The lines to draw in `width` cells, and the cursor's position among them.
    pub fn lines(&self, width: u16) -> (Vec<Line<'a>>, Position) {
        let indent = self.prefix.iter().map(Span::width).sum::<usize>();
        let gutter = |n: usize| match n {
            0 => self.prefix.clone(),
            _ => vec![Span::raw(" ".repeat(indent))],
        };
        let at = |row: usize, col: usize| Position::new((indent + col) as u16, row as u16);
        if self.field.is_empty() {
            let mut spans = gutter(0);
            spans.push(Span::raw(self.placeholder).dim());
            return (vec![Line::from(spans)], at(0, 0));
        }
        // One cell is kept free so the cursor fits after the last character.
        let layout = self
            .field
            .layout((width as usize).saturating_sub(indent + 1));
        let (row, col) = layout.cursor;
        let skip = layout
            .lines
            .len()
            .saturating_sub(self.max_lines)
            .min((row + 1).saturating_sub(self.max_lines));
        let lines = layout
            .lines
            .into_iter()
            .enumerate()
            .skip(skip)
            .take(self.max_lines)
            .map(|(n, pieces)| {
                let mut spans = gutter(n);
                spans.extend(pieces.into_iter().map(|p| match p.paste {
                    true => Span::styled(p.text, self.style.fg(Color::Cyan)),
                    false => Span::styled(p.text, self.style),
                }));
                Line::from(spans)
            })
            .collect();
        (lines, at(row - skip, col))
    }

    pub fn height(&self, width: u16) -> u16 {
        self.lines(width).0.len() as u16
    }

    /// Draws the field into `area` and, when it has focus, puts the terminal cursor in it.
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let (lines, cursor) = self.lines(area.width);
        frame.render_widget(Paragraph::new(lines), area);
        if focused {
            frame.set_cursor_position((area.x + cursor.x, area.y + cursor.y));
        }
    }

    /// The field on one unwrapped line with the cursor drawn as a bar, for places the terminal
    /// cursor can't be put, such as a table cell.
    pub fn line_with_bar(&self) -> Line<'a> {
        let (before, after) = self.field.shown_around_cursor();
        let mut spans = self.prefix.clone();
        spans.extend([
            Span::styled(before, self.style),
            Span::styled(CURSOR_BAR, self.style),
            Span::styled(after, self.style),
        ]);
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn an_empty_field_shows_its_placeholder_after_the_prefix() {
        let field = TextField::default();
        let (lines, cursor) = Input::new(&field)
            .prefix("❯ ")
            .placeholder("describe a task")
            .lines(40);
        assert_eq!(texts(&lines), ["❯ describe a task"]);
        assert_eq!(cursor, Position::new(2, 0));
    }

    #[test]
    fn wrapped_lines_are_indented_under_the_prefix() {
        let field = TextField::new("fix the parser now");
        let (lines, cursor) = Input::new(&field).prefix("> ").lines(12);
        assert_eq!(texts(&lines), ["> fix the ", "  parser ", "  now"]);
        assert_eq!(cursor, Position::new(5, 2));
    }

    #[test]
    fn past_max_lines_the_view_follows_the_cursor() {
        let mut field = TextField::new("a\nb\nc\nd");
        let (lines, _) = Input::new(&field).max_lines(2).lines(10);
        assert_eq!(texts(&lines), ["c", "d"]);

        for _ in 0..2 {
            field.apply(KeyEvent::from(KeyCode::Left));
        }
        let (lines, cursor) = Input::new(&field).max_lines(2).lines(10);
        assert_eq!(texts(&lines), ["b", "c"]);
        assert_eq!(cursor, Position::new(1, 1));
    }

    #[test]
    fn a_one_line_view_draws_the_cursor_as_a_bar() {
        let mut field = TextField::one_line("credit notes");
        field.apply(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(
            Input::new(&field).line_with_bar().to_string(),
            "credit ▏notes"
        );
    }
}
