use std::{
    num::NonZeroU16,
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line as TextLine, Span},
    widgets::{
        Block, Borders, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row, Table, Wrap,
    },
};

use crate::{
    agents::State,
    app::{App, Choice, Focus, Line},
    launch::{Draft, Field, Repo, Target},
    picker::{Item, Picker},
    place::{Place, Scope},
    pr::{Pr, PrState},
};

const PLACEHOLDER: &str = "describe a task for a new session";
// Nerd Font octicon git-branch; Ghostty bundles the Nerd Font symbols.
const WORKTREE: &str = "\u{f418}";
// Nerd Font octicons for pull request states.
const PR_OPEN: &str = "\u{f407}";
const PR_DRAFT: &str = "\u{f4dd}";
const PR_MERGED: &str = "\u{f419}";
const PR_CLOSED: &str = "\u{f4dc}";

pub fn draw(frame: &mut Frame, app: &mut App) {
    let details_height = if frame.area().height >= DETAILS_FROM_HEIGHT {
        DETAILS_HEIGHT
    } else {
        0
    };
    let [details, body, prompt, hints] = Layout::vertical([
        Constraint::Length(details_height),
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .areas(frame.area());
    if details_height > 0 {
        draw_details(frame, app, details);
    }
    if body.width >= MENU_FROM_WIDTH {
        let [menu, list] =
            Layout::horizontal([Constraint::Length(MENU_WIDTH), Constraint::Fill(1)]).areas(body);
        draw_menu(frame, app, menu);
        draw_sessions(frame, app, list);
    } else {
        draw_sessions(frame, app, body);
    }
    draw_prompt(frame, app, prompt);
    draw_hints(frame, app, hints);
    if let Some(draft) = &app.draft {
        draw_draft(frame, app, draft);
    }
    if let Some(choice) = &app.choosing {
        draw_choice(frame, app, choice);
    }
}

fn open_with(app: &App, i: usize) -> String {
    let names: Vec<&str> = app
        .fitting
        .get(i)
        .into_iter()
        .flatten()
        .map(|&o| app.openers[o].name.as_str())
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("   open with {} · ctrl+o", names.join(", "))
    }
}

fn draw_choice(frame: &mut Frame, app: &App, choice: &Choice) {
    let mut lines: Vec<TextLine> = choice
        .options
        .iter()
        .enumerate()
        .map(|(n, &o)| {
            let line = TextLine::from(vec![
                Span::raw(format!(" {} ", n + 1)).dim(),
                Span::raw(app.openers[o].name.clone()),
            ]);
            if n == choice.cursor {
                line.patch_style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect();
    lines.push(TextLine::default());
    lines.push(TextLine::from(" ↑↓ and enter, or a number · esc to cancel").dim());
    let width = lines
        .iter()
        .map(|l| l.width() as u16)
        .max()
        .unwrap_or(20)
        .max(36)
        + 4;
    let area = centered(frame.area(), width, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(format!(" open {} ", tilde(&choice.dir.to_string_lossy())).bold())
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
}

const MENU_WIDTH: u16 = 28;
const MENU_FROM_WIDTH: u16 = 90;

fn draw_menu(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Menu;
    let block = Block::bordered()
        .title(" projects ".dim())
        .border_style(if focused {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new()
        });
    let lines: Vec<TextLine> = app
        .visible_scopes()
        .into_iter()
        .map(|scope| {
            let (indent, label, style) = match scope {
                Scope::All => ("", tilde(&app.base.to_string_lossy()), Style::new()),
                Scope::Org(org) => (
                    if app.collapsed.orgs.contains(org) {
                        "▸ "
                    } else {
                        "▾ "
                    },
                    org.clone(),
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Scope::Repo { repo, .. } => ("    ", repo.clone(), Style::new()),
                Scope::Outside => (
                    "! ",
                    "not in a repo".to_string(),
                    Style::new().fg(Color::Yellow),
                ),
            };
            let line = TextLine::from(vec![
                Span::raw(indent),
                Span::styled(label, style),
                Span::raw(format!(" {}", app.scope_count(scope))).dim(),
            ]);
            match (*scope == app.scope, focused) {
                (true, true) => line.patch_style(Style::new().add_modifier(Modifier::REVERSED)),
                (true, false) => line.patch_style(Style::new().fg(Color::Cyan)),
                _ => line,
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_sessions(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut state = std::mem::take(&mut app.table);
    let view: &App = app;
    let title = TextLine::from(vec![
        " ratatosk ".bold(),
        Span::raw(format!("{} sessions · ", view.sessions.len())).dim(),
        Span::styled(
            format!("{} need you", view.count(State::Blocked)),
            state_style(State::Blocked),
        ),
        Span::raw(" · ").dim(),
        Span::styled(
            format!("{} working ", view.count(State::Working)),
            state_style(State::Working),
        ),
    ]);

    let rows = view.lines.iter().map(|line| match line {
        Line::Header(group) => {
            let collapsed = view.collapsed.groups.contains(group);
            Row::new(vec![Cell::from(TextLine::from(vec![
                Span::styled(
                    format!("{} {group}", if collapsed { "▸" } else { "▾" }),
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::raw(if collapsed {
                    format!("  {}", view.group_count(group))
                } else {
                    String::new()
                })
                .dim(),
            ]))])
        }
        Line::Session(i) => {
            let session = &view.sessions[*i];
            Row::new(vec![
                Cell::from(TextLine::from(vec![
                    Span::raw("  "),
                    Span::styled(session.state.glyph(), state_style(session.state)),
                    Span::raw(" "),
                    match &view.renaming {
                        Some((id, buffer)) if *id == session.id => Span::styled(
                            format!("{buffer}▏"),
                            Style::new()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::UNDERLINED),
                        ),
                        _ => Span::raw(session.name.as_str()),
                    },
                ])),
                Cell::from(if view.places[*i].worktree().is_some() {
                    WORKTREE
                } else {
                    ""
                })
                .fg(Color::Cyan),
                Cell::from(prs_badge(&view.prs_for(*i))),
                Cell::from(age(session.started_at_ms)).dim(),
            ])
        }
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Length(8),
            Constraint::Length(5),
        ],
    )
    .block(
        Block::bordered()
            .title(title)
            .border_style(if view.focus == Focus::List {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new()
            }),
    )
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD))
    .highlight_symbol("❯ ")
    .highlight_spacing(HighlightSpacing::Always);
    frame.render_stateful_widget(table, area, &mut state);
    let offset = state.offset();
    app.table = state;
    let rows = area.height.saturating_sub(2) as usize;
    for (n, line) in app.lines.iter().enumerate().skip(offset).take(rows) {
        if let Line::Session(i) = line
            && let [pr] = app.prs_for(*i).as_slice()
        {
            link(
                frame.buffer_mut(),
                area,
                area.y + 1 + (n - offset) as u16,
                &pr_text(pr),
                &pr.url,
            );
        }
    }
}

// Border, location, folder, PRs and two status lines; fixed so the list doesn't jump.
const DETAILS_HEIGHT: u16 = 7;
const DETAILS_FROM_HEIGHT: u16 = 20;
const LABEL: usize = 9;

fn draw_details(frame: &mut Frame, app: &App, area: Rect) {
    let Some(i) = app.selected_session_index() else {
        frame.render_widget(Block::bordered().title(" no session selected ".dim()), area);
        return;
    };
    let session = &app.sessions[i];
    let block = Block::bordered()
        .title(TextLine::from(vec![
            Span::raw(format!(" {} ", session.name)).bold(),
            Span::raw(
                app.claude_names
                    .get(&session.id)
                    .map(|claude| format!("(Claude: {claude}) "))
                    .unwrap_or_default(),
            )
            .dim(),
            Span::styled(
                format!("{} ", state_word(session.state)),
                state_style(session.state),
            ),
        ]))
        .title(
            TextLine::from(match age(session.started_at_ms) {
                age if age.is_empty() => format!(" {} ", session.id),
                age => format!(" started {age} ago · {} ", session.id),
            })
            .dim()
            .right_aligned(),
        )
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let label = |text: &'static str| Span::raw(format!("{text:<LABEL$}")).dim();

    let location = match &app.places[i] {
        Place::Repo {
            org,
            repo,
            worktree,
        } => {
            let mut spans = vec![
                label("repo"),
                Span::raw(format!("{org}/{repo}")),
                Span::raw("   "),
            ];
            spans.push(match worktree {
                Some(name) => Span::raw(format!("{WORKTREE} {name}")).cyan(),
                None => Span::raw("main checkout").dim(),
            });
            if let Some(branch) = &app.branches[i] {
                spans.extend([
                    Span::raw("   "),
                    Span::raw("branch ").dim(),
                    Span::raw(branch.clone()),
                ]);
            }
            TextLine::from(spans)
        }
        Place::Outside => TextLine::from(vec![
            label("repo"),
            Span::raw("not in a repo").yellow(),
            Span::raw(" · started outside the repos, so no repo skills or settings").dim(),
        ]),
    };
    let mut lines = vec![
        location,
        TextLine::from(vec![
            label("folder"),
            Span::raw(tilde(&session.cwd.to_string_lossy())),
            Span::raw(open_with(app, i)).dim(),
        ]),
    ];
    let prs = app.prs_for(i);
    if !prs.is_empty() {
        let mut spans = vec![label(if prs.len() == 1 { "pr" } else { "prs" })];
        for (n, pr) in prs.iter().enumerate() {
            if n > 0 {
                spans.push(Span::raw("   "));
            }
            spans.extend(pr_badge(pr).spans);
            spans.push(Span::styled(
                format!(" {}", pr_word(pr.state)),
                pr_style(pr.state),
            ));
            if prs.len() == 1 && !pr.title.is_empty() {
                spans.push(Span::raw(format!(" · {}", pr.title)));
            }
        }
        lines.push(TextLine::from(spans));
    } else {
        lines.push(TextLine::default());
    }
    if let Some(detail) = &session.detail {
        let width = (inner.width as usize).saturating_sub(LABEL);
        let heading = if session.state == State::Blocked {
            "waiting"
        } else {
            "now"
        };
        for (n, text) in wrap_clipped(detail, width, 2).into_iter().enumerate() {
            let head = if n == 0 {
                label(heading)
            } else {
                Span::raw(" ".repeat(LABEL))
            };
            lines.push(TextLine::from(vec![head, Span::raw(text)]));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
    for pr in &prs {
        link(frame.buffer_mut(), area, inner.y + 2, &pr_text(pr), &pr.url);
    }
}

// Ratatui has no hyperlinks: the OSC 8 sequence and its text go into the first cell, forced to
// the text's width so the renderer skips the cells it covers. Ghostty opens it on Cmd+click.
fn link(buf: &mut Buffer, area: Rect, y: u16, text: &str, url: &str) {
    if url.is_empty() || y < area.top() || y >= area.bottom() {
        return;
    }
    let wanted: Vec<String> = text.chars().map(String::from).collect();
    let Some(width) = NonZeroU16::new(wanted.len() as u16) else {
        return;
    };
    let xs: Vec<u16> = (area.left()..area.right()).collect();
    let Some(start) = xs
        .windows(wanted.len())
        .find(|cells| {
            cells
                .iter()
                .zip(&wanted)
                .all(|(&x, w)| buf[(x, y)].symbol() == w)
        })
        .map(|cells| cells[0])
    else {
        return;
    };
    buf[(start, y)]
        .set_symbol(&format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\"))
        .set_diff_option(CellDiffOption::ForcedWidth(width));
}

fn wrap_clipped(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if Span::raw(candidate.as_str()).width() <= width {
            current = candidate;
            continue;
        }
        if lines.len() + 1 == max_lines {
            lines.push(ellipsize(&candidate, width));
            return lines;
        }
        lines.push(std::mem::replace(&mut current, word.to_string()));
        if words.peek().is_none() && lines.len() == max_lines {
            break;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn ellipsize(text: &str, width: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if Span::raw(format!("{out}{c}…")).width() > width {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

fn state_word(state: State) -> &'static str {
    match state {
        State::Working => "working",
        State::Blocked => "needs you",
        State::Done => "done",
        State::Unknown => "unknown",
    }
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::new().borders(Borders::TOP | Borders::BOTTOM).dim();
    let inner = block.inner(area);
    let room = (inner.width as usize).saturating_sub(3);
    let shown = if app.input.is_empty() {
        Span::raw(PLACEHOLDER).dim()
    } else if Span::raw(app.input.as_str()).width() <= room {
        Span::raw(app.input.clone())
    } else {
        let tail: String = app
            .input
            .chars()
            .rev()
            .take(room.saturating_sub(1))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Span::raw(format!("…{tail}"))
    };
    let typed = if app.input.is_empty() {
        0
    } else {
        shown.width() as u16
    };
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(TextLine::from(vec![Span::raw("❯ ").dim(), shown])),
        inner,
    );
    if app.draft.is_none() {
        frame.set_cursor_position((inner.x + 2 + typed, inner.y));
    }
}

fn draw_hints(frame: &mut Frame, app: &App, area: Rect) {
    let message = |text: &str, style: Style| {
        Paragraph::new(text.to_string())
            .style(style)
            .wrap(Wrap { trim: true })
    };
    if let Some(err) = &app.error {
        frame.render_widget(message(err, Style::new().fg(Color::Red)), area);
        return;
    }
    if let Some(status) = &app.status {
        frame.render_widget(message(status, Style::new().fg(Color::Yellow)), area);
        return;
    }
    let hints: [&str; 2] = if app.renaming.is_some() {
        [
            "type the new name · enter to save · esc to cancel",
            "an empty name goes back to Claude's own name",
        ]
    } else if app.focus == Focus::Menu {
        [
            "↑↓ to pick a project · the list follows · ← collapses an org, → opens it",
            "tab, enter, or → on anything else goes back to the list",
        ]
    } else if app.input.is_empty() {
        [
            "↑↓ to select · enter or → to attach · ←/→ on a group collapses/expands · tab for projects",
            "ctrl+o to open · ctrl+r to rename · ctrl+x to stop, twice to delete · type to start a session · esc to quit",
        ]
    } else {
        ["enter to review and start", "esc to clear"]
    };
    frame.render_widget(
        Paragraph::new(hints.map(TextLine::from).to_vec()).dim(),
        area,
    );
}

const PICKER_ROWS: usize = 12;

fn draw_draft(frame: &mut Frame, app: &App, draft: &Draft) {
    let repos = &app.repos;
    let target = draft.target.as_ref();
    let launch = draft.launch(&app.base, repos);
    let picking = draft.field == Field::Project;

    let field = |which: Field, label: &str, value: Option<String>| {
        let focused = draft.field == which;
        let value = match value {
            Some(v) if focused && which != Field::Project => Span::styled(
                format!("‹ {v} ›"),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Some(v) => Span::styled(v, Style::new().add_modifier(Modifier::BOLD)),
            None => Span::styled("pick one", Style::new().fg(Color::Yellow)),
        };
        TextLine::from(vec![
            Span::raw(if focused { "❯ " } else { "  " }),
            Span::raw(format!("{label:<9}")).dim(),
            value,
        ])
    };

    let mut lines = vec![
        TextLine::from(vec![
            Span::raw("  prompt   ").dim(),
            Span::raw(draft.prompt.as_str()),
        ]),
        TextLine::default(),
        field(Field::Project, "project", target.map(|t| t.label(repos))),
    ];
    let mut cursor = None;
    if picking {
        let filter_line = TextLine::from(vec![
            Span::raw("           "),
            Span::styled("filter ", Style::new().fg(Color::Cyan)),
            Span::raw(draft.picker.filter.as_str()),
        ]);
        cursor = Some((
            lines.len(),
            18 + TextLine::from(draft.picker.filter.as_str()).width(),
        ));
        lines.push(filter_line);
        lines.extend(picker_lines(&draft.picker, repos));
    }
    lines.push(field(
        Field::Model,
        "model",
        draft.model.map(|m| m.label().to_string()),
    ));
    lines.push(field(
        Field::Effort,
        "effort",
        draft.effort.map(|e| e.label().to_string()),
    ));
    lines.push(TextLine::default());
    let starts_in = match (&launch, target) {
        (Some(l), _) => format!(
            "{}{}",
            tilde(&l.dir.to_string_lossy()),
            if l.create { "  (new folder)" } else { "" }
        ),
        (None, Some(Target::Repo(i))) => tilde(&repos[*i].dir.to_string_lossy()),
        _ => String::new(),
    };
    lines.push(TextLine::from(vec![
        Span::raw("  starts in ").dim(),
        Span::raw(starts_in),
    ]));
    lines.push(TextLine::from(vec![
        Span::raw("  runs      ").dim(),
        Span::raw(
            launch
                .map(|l| format!("claude {}", shell_words(&l.args())))
                .unwrap_or_default(),
        )
        .dim(),
    ]));
    if draft.manual(repos) {
        lines.push(
            TextLine::from(
                "  pick the model and effort yourself for this project (manualModel in the config)",
            )
            .yellow(),
        );
    }
    if let Some(notice) = &draft.notice {
        lines.push(TextLine::from(format!("  {notice}")).red());
    }
    lines.push(TextLine::default());
    lines.push(
        TextLine::from(if picking {
            "  type to filter · ↑↓ move · → open · ← close · enter to pick · tab to next field · esc back"
        } else {
            "  ↑↓ or tab field · ←→ change · enter to start · ctrl+enter to start and open · esc back"
        })
        .dim(),
    );

    let area = centered(frame.area(), 100, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Start this session? ".bold())
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
    if let Some((row, col)) = cursor {
        frame.set_cursor_position((area.x + 1 + col as u16, area.y + 1 + row as u16));
    }
}

fn picker_lines<'a>(picker: &Picker, repos: &'a [Repo]) -> Vec<TextLine<'a>> {
    let items = picker.items(repos);
    if items.is_empty() {
        return vec![
            TextLine::from("             no matches; type a folder name to create one").dim(),
        ];
    }
    let first = picker
        .cursor
        .saturating_sub(PICKER_ROWS - 1)
        .min(items.len().saturating_sub(PICKER_ROWS));
    items
        .iter()
        .enumerate()
        .skip(first)
        .take(PICKER_ROWS)
        .map(|(i, item)| {
            let line = match item {
                Item::Heading(text) => TextLine::from(format!("           {text}"))
                    .style(Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC)),
                Item::Recent(r) => TextLine::from(format!("             {}", repos[*r].label())),
                Item::Org { name, count, open } => TextLine::from(vec![
                    Span::raw(format!("             {} ", if *open { "▾" } else { "▸" })),
                    Span::styled(name.clone(), Style::new().fg(Color::Yellow)),
                    Span::raw(format!("  {count}")).dim(),
                ]),
                Item::Repo(r) if picker.filtering() => {
                    TextLine::from(format!("             {}", repos[*r].label()))
                }
                Item::Repo(r) => TextLine::from(format!("                 {}", repos[*r].name)),
                Item::Create { org, name } => {
                    TextLine::from(format!("             + new folder {org}/{name}"))
                        .style(Style::new().fg(Color::Green))
                }
            };
            if i == picker.cursor {
                line.patch_style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

fn shell_words(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(char::is_whitespace) || a.is_empty() {
                format!("{a:?}")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn prs_badge(prs: &[Pr]) -> TextLine<'static> {
    match prs {
        [] => TextLine::default(),
        [pr] => pr_badge(pr),
        many => {
            let lead = [
                PrState::Open,
                PrState::Draft,
                PrState::Merged,
                PrState::Closed,
                PrState::Unknown,
            ]
            .into_iter()
            .find(|state| many.iter().any(|pr| pr.state == *state))
            .unwrap_or(PrState::Unknown);
            TextLine::from(format!("{} PRs", many.len())).style(pr_style(lead))
        }
    }
}

fn pr_text(pr: &Pr) -> String {
    let icon = match pr.state {
        PrState::Open | PrState::Unknown => PR_OPEN,
        PrState::Draft => PR_DRAFT,
        PrState::Merged => PR_MERGED,
        PrState::Closed => PR_CLOSED,
    };
    format!("{icon} {}", pr.number)
}

fn pr_badge(pr: &Pr) -> TextLine<'static> {
    TextLine::from(pr_text(pr)).style(pr_style(pr.state))
}

fn pr_style(state: PrState) -> Style {
    match state {
        PrState::Open => Style::new().fg(Color::Green),
        PrState::Draft => Style::new().fg(Color::DarkGray),
        PrState::Merged => Style::new().fg(Color::Magenta),
        PrState::Closed => Style::new().fg(Color::Red),
        PrState::Unknown => Style::new().add_modifier(Modifier::DIM),
    }
}

fn pr_word(state: PrState) -> &'static str {
    match state {
        PrState::Open => "open",
        PrState::Draft => "draft",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
        PrState::Unknown => "state unknown",
    }
}

fn state_style(state: State) -> Style {
    match state {
        State::Working => Style::new().fg(Color::Green),
        State::Blocked => Style::new().fg(Color::Yellow),
        State::Done | State::Unknown => Style::new().fg(Color::DarkGray),
    }
}

fn age(started_at_ms: Option<u64>) -> String {
    let Some(started) = started_at_ms else {
        return String::new();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(started, |d| d.as_millis() as u64);
    let minutes = now.saturating_sub(started) / 60_000;
    match minutes {
        m if m < 60 => format!("{m}m"),
        m if m < 60 * 24 => format!("{}h", m / 60),
        m => format!("{}d", m / (60 * 24)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend, widgets::TableState};

    use super::*;
    use crate::agents::Session;

    fn app() -> App {
        App {
            base: PathBuf::from("/p"),
            sessions: vec![Session {
                id: "a1".into(),
                name: "billing: credit-note".into(),
                state: State::Done,
                cwd: PathBuf::from("/p/acme/billing/.claude/worktrees/credit-note-api"),
                started_at_ms: None,
                detail: Some("awaiting go-ahead to push".into()),
                prs: Vec::new(),
            }],
            places: vec![Place::Repo {
                org: "acme".into(),
                repo: "billing".into(),
                worktree: Some("credit-note-api".into()),
            }],
            branches: vec![Some("credit-note-api".into())],
            lines: vec![Line::Header("acme / billing".into()), Line::Session(0)],
            table: TableState::default().with_selected(Some(1)),
            error: None,
            repos: vec![Repo {
                org: "globex".into(),
                name: "engine".into(),
                dir: PathBuf::from("/p/globex/engine"),
            }],
            input: String::new(),
            draft: None,
            recent_path: None,
            status: None,
            armed_delete: None,
            focus: Focus::List,
            scopes: vec![Scope::All],
            scope: Scope::All,
            manual_model: Default::default(),
            ..Default::default()
        }
    }

    fn screen(app: &mut App) -> String {
        screen_at(app, 120)
    }

    fn screen_at(app: &mut App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn group_and_full_session_names_are_drawn() {
        let screen = screen(&mut app());
        assert!(screen.contains("acme / billing"), "{screen}");
        assert!(screen.contains("billing: credit-note"), "{screen}");
        assert!(screen.contains(WORKTREE), "{screen}");
        assert!(screen.contains(PLACEHOLDER), "{screen}");
    }

    #[test]
    fn details_show_what_the_session_waits_for_and_where_it_runs() {
        let screen = screen(&mut app());
        assert!(screen.contains("awaiting go-ahead to push"), "{screen}");
        assert!(screen.contains("acme/billing"), "{screen}");
        assert!(screen.contains("credit-note-api"), "{screen}");
        assert!(screen.contains("a1"), "{screen}");
    }

    #[test]
    fn a_linked_pr_shows_in_the_list_and_details() {
        let mut app = app();
        app.dir_repos
            .insert(PathBuf::from("/p/acme/billing"), "acme/billing".into());
        app.prs.insert(
            "acme/billing".into(),
            crate::pr::RepoPrs {
                repo: "acme/billing".into(),
                by_branch: [("credit-note-api".to_string(), 2085)].into(),
                by_number: [(
                    2085,
                    Pr {
                        number: 2085,
                        state: PrState::Open,
                        title: "Track payouts".into(),
                        url: "https://github.com/acme/billing/pull/2085".into(),
                    },
                )]
                .into(),
            },
        );
        let screen = screen(&mut app);
        assert!(screen.contains(&format!("{PR_OPEN} 2085")), "{screen}");
        assert!(screen.contains("open · Track payouts"), "{screen}");
        let link = "\x1b]8;;https://github.com/acme/billing/pull/2085\x1b\\";
        assert_eq!(
            screen.matches(link).count(),
            2,
            "list and details both link the PR"
        );
    }

    #[test]
    fn several_linked_prs_show_as_a_count() {
        let mut app = app();
        app.sessions[0].prs = vec![
            crate::agents::PrRef {
                repo: "acme/billing".into(),
                number: 1,
            },
            crate::agents::PrRef {
                repo: "acme/billing".into(),
                number: 2,
            },
            crate::agents::PrRef {
                repo: "acme/billing".into(),
                number: 3,
            },
        ];
        let screen = screen(&mut app);
        assert!(screen.contains("3 PRs"), "{screen}");
        assert!(screen.contains("state unknown"), "{screen}");
    }

    #[test]
    fn long_status_is_cut_to_two_lines() {
        let lines = wrap_clipped(&"word ".repeat(40), 30, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(
            lines.iter().all(|l| Span::raw(l.as_str()).width() <= 30),
            "{lines:?}"
        );
        assert_eq!(wrap_clipped("short status", 30, 2), ["short status"]);
    }

    #[test]
    fn a_long_prompt_shows_its_end() {
        let mut app = app();
        app.input = format!("{} the end", "x".repeat(200));
        let screen = screen(&mut app);
        assert!(screen.contains("…"), "{screen}");
        assert!(screen.contains("the end"), "{screen}");
    }

    #[test]
    fn project_picker_shows_filter_tree_and_new_folder_option() {
        let mut app = app();
        app.input = "start a tool".into();
        app.draft = Some(Draft::new(
            app.input.clone(),
            None,
            Vec::new(),
            &app.repos,
            Default::default(),
        ));
        let before = screen(&mut app);
        assert!(before.contains("filter"), "{before}");
        assert!(before.contains("all projects"), "{before}");
        assert!(before.contains("▸ globex"), "{before}");

        let repos = app.repos.clone();
        let draft = app.draft.as_mut().unwrap();
        for c in "newtool".chars() {
            draft.picker.type_char(&repos, c);
        }
        let after = screen(&mut app);
        assert!(after.contains("+ new folder globex/newtool"), "{after}");
    }

    #[test]
    fn confirmation_shows_the_command_it_will_run() {
        let mut app = app();
        app.input = "fix the parser".into();
        app.draft = Some(Draft::new(
            app.input.clone(),
            Some(Target::Repo(0)),
            Vec::new(),
            &app.repos,
            Default::default(),
        ));
        let screen = screen(&mut app);
        assert!(screen.contains("Start this session?"), "{screen}");
        assert!(screen.contains("globex/engine"), "{screen}");
        assert!(
            screen.contains("claude --bg \"fix the parser\""),
            "{screen}"
        );
    }
}

#[cfg(test)]
mod menu_tests {
    use std::path::{Path, PathBuf};

    use ratatui::{Terminal, backend::TestBackend, widgets::TableState};

    use super::*;
    use crate::agents::Session;

    fn session(id: &str, name: &str, cwd: &str) -> Session {
        Session {
            id: id.into(),
            name: name.into(),
            state: State::Done,
            cwd: PathBuf::from(cwd),
            started_at_ms: None,
            detail: None,
            prs: Vec::new(),
        }
    }

    #[test]
    fn menu_lists_projects_with_counts() {
        let sessions = vec![
            session("a", "billing: one", "/p/acme/billing"),
            session("b", "engine: two", "/p/globex/engine"),
        ];
        let places: Vec<Place> = sessions
            .iter()
            .map(|s| Place::of(Path::new("/p"), &s.cwd))
            .collect();
        let mut app = App {
            base: PathBuf::from("/p"),
            scopes: Scope::for_places(&places),
            branches: vec![None, None],
            lines: vec![Line::Header("acme / billing".into()), Line::Session(0)],
            places,
            sessions,
            table: TableState::default().with_selected(Some(1)),
            error: None,
            repos: Vec::new(),
            input: String::new(),
            draft: None,
            recent_path: None,
            status: None,
            armed_delete: None,
            manual_model: Default::default(),
            focus: Focus::Menu,
            scope: Scope::Org("acme".into()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("projects"), "{screen}");
        assert!(screen.contains("engine 1"), "{screen}");
        assert!(screen.contains("acme 1"), "{screen}");
        assert!(screen.contains("↑↓ to pick a project"), "{screen}");
    }
}
