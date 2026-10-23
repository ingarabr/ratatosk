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
    account,
    agents::State,
    app::{AccountPicker, App, Choice, Focus, Line},
    input::Input,
    launch::{Draft, Field, Repo, Target},
    picker::{Item, Picker},
    place::{Kind, Place, Scope},
    pr::{Pr, PrState},
};

const PLACEHOLDER: &str = "describe a task for a new session";
// Nerd Font octicon git-branch; Ghostty bundles the Nerd Font symbols.
const WORKTREE: &str = "\u{f418}";
// Nerd Font octicons for pull request states.
const PR_OPEN: &str = "\u{f407}";
// Nerd Font icons for what a group is: a GitHub repo, a local git repo, or a plain folder.
const ICON_GITHUB: &str = "\u{f408}";
const ICON_GIT: &str = "\u{e702}";
const ICON_FOLDER: &str = "\u{f413}";
const PR_DRAFT: &str = "\u{f4dd}";
const PR_MERGED: &str = "\u{f419}";
const PR_CLOSED: &str = "\u{f4dc}";

pub fn draw(frame: &mut Frame, app: &mut App) {
    let details_height = if frame.area().height >= DETAILS_FROM_HEIGHT {
        DETAILS_HEIGHT
    } else {
        0
    };
    let prompt_height = prompt_input(app).height(frame.area().width) + 2;
    let [details, body, prompt, hints] = Layout::vertical([
        Constraint::Length(details_height),
        Constraint::Fill(1),
        Constraint::Length(prompt_height),
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
        app.areas.menu = menu;
        app.areas.list = list;
    } else {
        draw_sessions(frame, app, body);
        app.areas.menu = Rect::default();
        app.areas.list = body;
    }
    app.areas.prompt = draw_prompt(frame, app, prompt);
    draw_hints(frame, app, hints);
    if let Some(draft) = &app.draft {
        draw_draft(frame, app, draft);
    }
    if let Some(choice) = &app.choosing {
        draw_choice(frame, app, choice);
    }
    if let Some(picker) = &app.picking_account {
        draw_accounts(frame, app, picker);
    }
    app.areas.paste_view = draw_paste_view(frame, app);
}

fn draw_accounts(frame: &mut Frame, app: &App, picker: &AccountPicker) {
    let Some(pool) = &app.pool else {
        return;
    };
    let active = app.signed_in.as_ref().map(|m| m.email.to_lowercase());
    let mut lines: Vec<TextLine> = pool
        .members
        .iter()
        .map(|m| {
            let current = Some(m.email.to_lowercase()) == active;
            TextLine::from(vec![
                Span::raw(if current { " ● " } else { "   " }).green(),
                Span::raw(m.email.clone()),
                Span::raw(
                    m.organization
                        .as_ref()
                        .map(|o| format!("  {o}"))
                        .unwrap_or_default(),
                )
                .dim(),
            ])
        })
        .collect();
    lines.push(TextLine::from("   + sign in to another account").cyan());
    if let Some(line) = lines.get_mut(picker.cursor) {
        *line = std::mem::take(line).patch_style(Style::new().add_modifier(Modifier::REVERSED));
    }
    lines.push(TextLine::default());
    lines.push(
        TextLine::from(" enter switches the default login · ctrl+x twice forgets one · esc").dim(),
    );
    lines.push(TextLine::from(" pinned orgs keep their own account").dim());
    let width = lines
        .iter()
        .map(|l| l.width() as u16)
        .max()
        .unwrap_or(20)
        .max(40)
        + 4;
    let area = centered(frame.area(), width, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" accounts ".bold())
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
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
                    if app.view.collapsed_orgs.contains(org) {
                        "▸ "
                    } else {
                        "▾ "
                    },
                    format!("{} {org}", kind_icon(org_kind(app, org))),
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Scope::Repo { org, repo } => {
                    let kind = app.places.iter().find_map(|p| match p {
                        Place::Repo {
                            org: o,
                            repo: r,
                            kind,
                            ..
                        } if o == org && r == repo => Some(*kind),
                        _ => None,
                    });
                    (
                        "    ",
                        format!("{} {repo}", kind_icon(kind.unwrap_or_default())),
                        Style::new(),
                    )
                }
                Scope::Loose(dir) => (
                    "  ",
                    format!("{ICON_FOLDER} {}", tilde(&dir.to_string_lossy())),
                    Style::new(),
                ),
                Scope::Elsewhere => ("  ", format!("{ICON_FOLDER} elsewhere"), Style::new()),
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
            format!("{} working", view.count(State::Working)),
            state_style(State::Working),
        ),
        Span::raw(format!(" · by {} ", view.view.sort.label())).dim(),
        Span::raw(
            view.signed_in
                .as_ref()
                .map(|m| format!("· {} ", m.email))
                .unwrap_or_default(),
        )
        .dim(),
    ]);

    let rows = view.lines.iter().map(|line| match line {
        Line::Header(group) => {
            let collapsed = view.view.collapsed_groups.contains(group);
            let icon = view
                .places
                .iter()
                .find(|p| p.group() == *group)
                .map_or(ICON_FOLDER, |p| match p {
                    Place::Repo { kind, .. } => kind_icon(*kind),
                    _ => ICON_FOLDER,
                });
            Row::new(vec![Cell::from(TextLine::from(vec![
                Span::styled(
                    format!("{} {icon} {group}", if collapsed { "▸" } else { "▾" }),
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
            let mut name = vec![
                Span::raw("  "),
                Span::styled(session.state.glyph(), state_style(session.state)),
                Span::raw(" "),
            ];
            match &view.renaming {
                Some((id, buffer)) if *id == session.id => name.extend(
                    Input::new(buffer)
                        .style(
                            Style::new()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::UNDERLINED),
                        )
                        .line_with_bar()
                        .spans,
                ),
                _ => name.push(Span::raw(session.name.as_str())),
            }
            if let Some(want) = view.misplaced(*i) {
                name.push(Span::raw(format!("  ⚠ not on {}", view.accounts.get(want).name)).red());
            }
            Row::new(vec![
                Cell::from(TextLine::from(name)),
                Cell::from(if view.places[*i].worktree().is_some() {
                    WORKTREE
                } else {
                    ""
                })
                .fg(Color::Cyan),
                Cell::from(prs_badge(&view.prs_for(*i))),
                Cell::from(age(session.active_at_ms)).style(recency_style(session.active_at_ms)),
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
            Span::raw(if session.interactive {
                "· in its own terminal "
            } else {
                ""
            })
            .dim(),
            Span::raw(if session.account == account::DEFAULT {
                String::new()
            } else {
                format!("· {} account ", app.accounts.get(session.account).name)
            })
            .dim(),
        ]))
        .title(
            TextLine::from(match age(session.active_at_ms) {
                age if age.is_empty() => format!(" {} ", session.id),
                age => format!(" active {age} ago · {} ", session.id),
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
            kind,
            ..
        } => {
            let mut spans = vec![
                label("repo"),
                Span::raw(format!("{} {org}/{repo}", kind_icon(*kind))),
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
        Place::Loose(dir) => TextLine::from(vec![
            label("repo"),
            Span::raw("none").yellow(),
            Span::raw(format!(
                " · started in {}, outside any project, so no repo skills or settings",
                tilde(&dir.to_string_lossy())
            ))
            .dim(),
        ]),
        Place::Elsewhere => TextLine::from(vec![
            label("repo"),
            Span::raw("none").yellow(),
            Span::raw(format!(
                " · started outside {}",
                tilde(&app.base.to_string_lossy())
            ))
            .dim(),
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

fn text_width(text: &str) -> usize {
    Span::raw(text).width()
}

// Wraps at spaces, keeps newlines, breaks words longer than a line, and cuts to `max_lines`
// with an ellipsis.
fn wrap_clipped(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word.to_string();
            loop {
                let candidate = if current.is_empty() {
                    word.clone()
                } else {
                    format!("{current} {word}")
                };
                if text_width(&candidate) <= width {
                    current = candidate;
                    break;
                }
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    continue;
                }
                let head: String = word
                    .chars()
                    .scan(0, |w, c| {
                        *w += text_width(&c.to_string());
                        (*w <= width).then_some(c)
                    })
                    .collect();
                word = word[head.len()..].to_string();
                lines.push(head);
            }
        }
        lines.push(current);
    }
    if lines.len() > max_lines {
        let rest = lines[max_lines].clone();
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = ellipsize(&format!("{last} {rest}"), width);
        }
    }
    lines
}

fn labelled(
    label: &str,
    text: &str,
    width: usize,
    max_lines: usize,
    style: Style,
) -> Vec<TextLine<'static>> {
    wrap_clipped(text, width.saturating_sub(label.len()), max_lines)
        .into_iter()
        .enumerate()
        .map(|(n, line)| {
            let head = if n == 0 {
                label.to_string()
            } else {
                " ".repeat(label.len())
            };
            TextLine::from(vec![Span::raw(head).dim(), Span::styled(line, style)])
        })
        .collect()
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

const PROMPT_MAX_LINES: usize = 8;

pub fn prompt_input(app: &App) -> Input<'_> {
    Input::new(&app.input)
        .prefix(Span::raw("❯ ").dim())
        .placeholder(PLACEHOLDER)
        .max_lines(PROMPT_MAX_LINES)
}

// Gives back where the text went, for clicks.
fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) -> Rect {
    let block = Block::new().borders(Borders::TOP | Borders::BOTTOM).dim();
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let focused = app.draft.is_none() && app.paste_view.is_none();
    prompt_input(app).render(frame, inner, focused);
    inner
}

const PASTE_VIEW_WIDTH: u16 = 110;

// Draws the open paste, numbered and wrapped in full, and keeps its scroll in range. Gives
// back where it went, for clicks.
fn draw_paste_view(frame: &mut Frame, app: &mut App) -> Rect {
    let Some(view) = app.paste_view else {
        return Rect::default();
    };
    let text = app.input.pasted(view.paste);
    let source: Vec<&str> = text.split('\n').collect();
    let number_width = source.len().to_string().len();
    let box_width = frame.area().width.min(PASTE_VIEW_WIDTH);
    let room = (box_width as usize).saturating_sub(number_width + 5).max(1);
    let mut rows: Vec<(usize, TextLine)> = Vec::new();
    for (n, line) in source.iter().enumerate() {
        let line = line.replace('\t', "    ");
        let mut chunks = hard_wrap(&line, room).into_iter();
        let first = chunks.next().unwrap_or_default();
        rows.push((
            n,
            TextLine::from(vec![
                Span::raw(format!(" {:>number_width$}  ", n + 1)).dim(),
                Span::raw(first),
            ]),
        ));
        for chunk in chunks {
            rows.push((
                n,
                TextLine::from(vec![
                    Span::raw(" ".repeat(number_width + 3)),
                    Span::raw(chunk),
                ]),
            ));
        }
    }
    let shown = rows
        .len()
        .min(frame.area().height.saturating_sub(7) as usize)
        .max(1);
    let scroll = view.scroll.min(rows.len().saturating_sub(shown));
    if let Some(view) = app.paste_view.as_mut() {
        view.scroll = scroll;
    }
    let visible = &rows[scroll..(scroll + shown).min(rows.len())];
    let (first, last) = (
        visible.first().map_or(0, |r| r.0),
        visible.last().map_or(0, |r| r.0),
    );
    let mut lines: Vec<TextLine> = visible.iter().map(|(_, line)| line.clone()).collect();
    lines.push(TextLine::default());
    lines.push(
        TextLine::from(format!(
            " lines {}–{} of {} · ↑↓ or the wheel to scroll · pgup/pgdn · esc back",
            first + 1,
            last + 1,
            source.len()
        ))
        .dim(),
    );
    lines.push(
        TextLine::from(
            " ⌫ removes it · enter puts it in the prompt as text · ctrl+g edits it in $EDITOR",
        )
        .dim(),
    );
    let title = format!(
        " Pasted text #{} · {} lines · {} ",
        view.paste + 1,
        source.len(),
        size(text.len())
    );
    let area = centered(frame.area(), box_width, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title.bold())
                .border_style(Style::new().fg(Color::Cyan)),
        ),
        area,
    );
    area
}

// Cuts at exactly `width` cells, keeping every character, spaces included.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let mut chunks = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = text_width(c.encode_utf8(&mut [0; 4]));
        if used + w > width && used > 0 {
            chunks.push(String::new());
            used = 0;
        }
        chunks.last_mut().unwrap().push(c);
        used += w;
    }
    chunks
}

fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b => format!("{:.1} kB", b as f64 / 1024.0),
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
            "←→ move · opt+←→ by word · opt+⌫ deletes a word · cmd+⌫ to line start · an empty name goes back to Claude's own name",
        ]
    } else if app.focus == Focus::Menu {
        [
            "↑↓ to pick a project · the list follows · ← collapses an org, → opens it",
            "tab, enter, or → on anything else goes back to the list",
        ]
    } else if app.input.is_empty() {
        [
            "↑↓ to select · enter or → to attach · ←/→ on a group collapses/expands · tab for projects",
            "ctrl+a for accounts · ctrl+o to open · ctrl+l to move · ctrl+r to rename · ctrl+s to sort · ctrl+x to stop, twice to delete · esc to quit",
        ]
    } else if app.input.paste_at_cursor().is_some() {
        [
            "ctrl+g or a click opens the pasted text to check it · enter to review and start",
            "←→ move · opt+⌫ deletes the paste · ctrl+u/k delete to start/end · ctrl+y puts it back",
        ]
    } else {
        [
            "enter to review and start · shift+enter (or ctrl+j) for a new line · ctrl+g opens $EDITOR · esc clears",
            "←→ move · opt+←→ by word · ctrl+a/e line start/end · opt+⌫ deletes a word · ctrl+u/k delete to start/end · ctrl+y puts it back",
        ]
    };
    frame.render_widget(
        Paragraph::new(hints.map(TextLine::from).to_vec()).dim(),
        area,
    );
}

const PICKER_ROWS: usize = 12;
const DRAFT_WIDTH: u16 = 100;

fn draw_draft(frame: &mut Frame, app: &App, draft: &Draft) {
    let repos = &app.repos;
    let target = draft.target.as_ref();
    let launch = draft.launch(&app.base, repos, &app.accounts);
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

    let box_width = frame.area().width.min(DRAFT_WIDTH);
    let text_room = box_width.saturating_sub(4) as usize;
    let mut lines = labelled("  prompt   ", &draft.shown, text_room, 6, Style::new());
    lines.push(TextLine::default());
    lines.push(field(
        Field::Project,
        "project",
        target.map(|t| t.label(repos)),
    ));
    let mut cursor = None;
    if picking {
        let (filter, at) = Input::new(&draft.picker.filter)
            .prefix(vec![
                Span::raw("           "),
                Span::styled("filter ", Style::new().fg(Color::Cyan)),
            ])
            .max_lines(1)
            .lines(text_room as u16);
        cursor = Some((lines.len(), at.x as usize));
        lines.extend(filter);
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
    if let Some(l) = &launch {
        let (name, why) = if l.account != account::DEFAULT {
            (
                app.accounts.get(l.account).name.clone(),
                "  (pinned for this org in the config)",
            )
        } else {
            (
                app.signed_in
                    .as_ref()
                    .map_or_else(|| "default login".to_string(), |m| m.email.clone()),
                "  (the default login · ctrl+a in the list switches it)",
            )
        };
        lines.push(TextLine::from(vec![
            Span::raw("  account   ").dim(),
            Span::raw(name),
            Span::raw(why).dim(),
        ]));
    }
    let command = launch
        .as_ref()
        .map(|l| {
            format!(
                "{}claude {}",
                app.accounts.get(l.account).env_prefix(),
                shell_words(&l.args())
            )
        })
        .unwrap_or_default();
    lines.extend(labelled(
        "  runs      ",
        &command,
        text_room,
        3,
        Style::new().add_modifier(Modifier::DIM),
    ));
    if draft.manual(repos) {
        lines.push(
            TextLine::from(
                "  pick the model and effort yourself for this project (manualModel in the config)",
            )
            .yellow(),
        );
    }
    if let Some(notice) = &draft.notice {
        let color = if draft.needs_trust {
            Color::Yellow
        } else {
            Color::Red
        };
        lines.extend(labelled("  ", notice, text_room, 4, Style::new().fg(color)));
    }
    lines.push(TextLine::default());
    lines.push(
        TextLine::from(if picking {
            "  type to filter · ↑↓ move · → open · ← close · enter to pick · tab to next field · esc back"
        } else {
            "  ↑↓ or tab field · ←→ change · enter to start (or trust) · ctrl+enter to start and open · esc back"
        })
        .dim(),
    );

    let area = centered(frame.area(), box_width, lines.len() as u16 + 2);
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
    crate::place::tilde(std::path::Path::new(path))
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

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::GitHub => ICON_GITHUB,
        Kind::Git => ICON_GIT,
        Kind::Folder => ICON_FOLDER,
    }
}

fn org_kind(app: &App, org: &str) -> Kind {
    let kinds: Vec<Kind> = app
        .places
        .iter()
        .filter_map(|p| match p {
            Place::Repo { org: o, kind, .. } if o == org => Some(*kind),
            _ => None,
        })
        .collect();
    [Kind::GitHub, Kind::Git]
        .into_iter()
        .find(|k| kinds.contains(k))
        .unwrap_or(Kind::Folder)
}

fn state_style(state: State) -> Style {
    match state {
        State::Working => Style::new().fg(Color::Green),
        State::Blocked => Style::new().fg(Color::Yellow),
        State::Done | State::Unknown => Style::new().fg(Color::DarkGray),
    }
}

fn minutes_since(at_ms: Option<u64>) -> Option<u64> {
    let at = at_ms?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(at, |d| d.as_millis() as u64);
    Some(now.saturating_sub(at) / 60_000)
}

fn age(at_ms: Option<u64>) -> String {
    match minutes_since(at_ms) {
        None => String::new(),
        Some(m) if m < 60 => format!("{m}m"),
        Some(m) if m < 60 * 24 => format!("{}h", m / 60),
        Some(m) => format!("{}d", m / (60 * 24)),
    }
}

const FADE_MINUTES: f64 = 7.0 * 24.0 * 60.0;

// Bright for recent activity, fading on a log scale to grey over a week; older is dim.
fn recency_style(at_ms: Option<u64>) -> Style {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let Some(minutes) = minutes_since(at_ms) else {
        return dim;
    };
    if minutes as f64 >= FADE_MINUTES {
        return dim;
    }
    let t = ((1.0 + minutes as f64).ln() / (1.0 + FADE_MINUTES).ln()).clamp(0.0, 1.0);
    let mix =
        |from: u8, to: u8| (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8;
    let style = Style::new().fg(Color::Rgb(mix(120, 110), mix(220, 118), mix(255, 128)));
    if minutes < 60 {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend, widgets::TableState};

    use super::*;
    use crate::{agents::Session, app::Action};

    fn app() -> App {
        App {
            base: PathBuf::from("/p"),
            sessions: vec![Session {
                id: "a1".into(),
                name: "billing: credit-note".into(),
                state: State::Done,
                cwd: PathBuf::from("/p/acme/billing/.claude/worktrees/credit-note-api"),
                started_at_ms: None,
                active_at_ms: None,
                detail: Some("awaiting go-ahead to push".into()),
                prs: Vec::new(),
                interactive: false,
                account: 0,
                session_id: None,
                running: false,
            }],
            places: vec![crate::place::repo(
                "acme",
                "billing",
                Some("credit-note-api"),
            )],
            branches: vec![Some("credit-note-api".into())],
            lines: vec![Line::Header("acme / billing".into()), Line::Session(0)],
            table: TableState::default().with_selected(Some(1)),
            error: None,
            repos: vec![Repo {
                org: "globex".into(),
                name: "engine".into(),
                dir: PathBuf::from("/p/globex/engine"),
                kind: crate::place::Kind::Folder,
                folder_org: "globex".into(),
                folder: "engine".into(),
            }],
            input: Default::default(),
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
    fn recent_activity_is_bright_and_old_activity_dim() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let minutes_ago = |m: u64| Some(now - m * 60_000);
        let fresh = recency_style(minutes_ago(5));
        assert!(fresh.add_modifier.contains(Modifier::BOLD));
        let Some(Color::Rgb(r1, ..)) = recency_style(minutes_ago(60 * 3)).fg else {
            panic!("hours get a colour")
        };
        let Some(Color::Rgb(r2, ..)) = recency_style(minutes_ago(60 * 24 * 5)).fg else {
            panic!("days get a colour")
        };
        assert_ne!(r1, r2, "the colour changes with age");
        assert!(
            recency_style(minutes_ago(60 * 24 * 8))
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(recency_style(None).add_modifier.contains(Modifier::DIM));
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
    fn a_long_prompt_wraps_over_several_lines_and_keeps_newlines() {
        let mut app = app();
        app.input = format!("{} the end\nsecond line", "word ".repeat(60)).into();
        let (lines, cursor) = prompt_input(&app).lines(80);
        assert!(lines.len() > 2, "{lines:?}");
        assert_eq!(lines.last().unwrap().to_string(), "  second line");
        assert_eq!(
            cursor,
            ratatui::layout::Position::new(13, lines.len() as u16 - 1)
        );
        let screen = screen(&mut app);
        assert!(screen.contains("the end"), "{screen}");
        assert!(screen.contains("second line"), "{screen}");
    }

    #[test]
    fn wrapping_breaks_long_words_and_respects_newlines() {
        let lines = wrap_clipped(&format!("{}\nnext", "x".repeat(25)), 10, usize::MAX);
        assert_eq!(lines, ["xxxxxxxxxx", "xxxxxxxxxx", "xxxxx", "next"]);
    }

    #[test]
    fn the_confirmation_shows_a_long_message_in_full() {
        let mut app = app();
        app.input = "fix the parser".into();
        let mut draft = Draft::new(
            app.input.text().to_string(),
            Some(Target::Repo(0)),
            Vec::new(),
            &app.repos,
            Default::default(),
        );
        draft.needs_trust = true;
        draft.notice = Some(format!(
            "Claude Code doesn't trust {} yet. Enter opens Claude there: accept its trust prompt, type /exit, and the session starts.",
            "/p/a/very/long/path/to/a/folder/that/goes/on"
        ));
        app.draft = Some(draft);
        let screen = screen(&mut app);
        assert!(screen.contains("and the session starts."), "{screen}");
    }

    #[test]
    fn project_picker_shows_filter_tree_and_new_folder_option() {
        let mut app = app();
        app.input = "start a tool".into();
        app.draft = Some(Draft::new(
            app.input.text().to_string(),
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
        draft.picker.type_text(&repos, "newtool");
        let after = screen(&mut app);
        assert!(after.contains("+ new folder globex/newtool"), "{after}");
    }

    #[test]
    fn confirmation_shows_the_command_it_will_run() {
        let mut app = app();
        app.input = "fix the parser".into();
        app.draft = Some(Draft::new(
            app.input.text().to_string(),
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

    // Draws a frame and gives the screen spot where `text` starts.
    fn spot_of(app: &mut App, text: &str) -> (u16, u16) {
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        for y in 0..buffer.area.height {
            let cells: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            for x in 0..cells.len() {
                if cells[x..].concat().starts_with(text) {
                    return (x as u16, y);
                }
            }
        }
        panic!("{text:?} is not on screen");
    }

    fn click(app: &mut App, (column, row): (u16, u16), now: std::time::Instant) -> Action {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        app.on_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            now,
        )
    }

    #[test]
    fn a_click_selects_a_row_and_a_double_click_attaches() {
        let mut app = app();
        app.table.select(Some(0));
        let at = spot_of(&mut app, "billing: credit-note");
        let now = std::time::Instant::now();
        assert!(matches!(click(&mut app, at, now), Action::None));
        assert_eq!(app.table.selected(), Some(1));
        let later = now + std::time::Duration::from_millis(100);
        assert!(matches!(click(&mut app, at, later), Action::Attach { .. }));
    }

    #[test]
    fn a_click_on_a_paste_opens_it_numbered_in_full() {
        let mut app = app();
        app.on_paste("why ");
        let trace: Vec<String> = (1..=30).map(|n| format!("frame {n}")).collect();
        app.on_paste(&trace.join("\n"));
        let at = spot_of(&mut app, "[Pasted text #1 +30 lines]");
        click(&mut app, (at.0 + 3, at.1), std::time::Instant::now());
        assert!(app.paste_view.is_some());

        let top = screen(&mut app);
        assert!(top.contains("Pasted text #1 · 30 lines"), "{top}");
        assert!(top.contains(" 1  frame 1"), "{top}");
        assert!(!top.contains("frame 30"), "{top}");

        app.paste_view.as_mut().unwrap().scroll = usize::MAX;
        let bottom = screen(&mut app);
        assert!(bottom.contains("30  frame 30"), "{bottom}");
        assert!(bottom.contains("of 30"), "{bottom}");

        click(&mut app, (0, 0), std::time::Instant::now());
        assert!(app.paste_view.is_none());
    }

    #[test]
    fn the_hint_says_how_to_open_the_paste_at_the_cursor() {
        let mut app = app();
        app.on_paste("a\nb\nc\nd");
        let screen = screen(&mut app);
        assert!(
            screen.contains("ctrl+g or a click opens the pasted text"),
            "{screen}"
        );
    }

    #[test]
    fn long_lines_are_wrapped_whole() {
        assert_eq!(hard_wrap("ab  cd", 3), ["ab ", " cd"]);
        assert_eq!(hard_wrap("", 3), [""]);
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
            active_at_ms: None,
            detail: None,
            prs: Vec::new(),
            interactive: false,
            account: 0,
            session_id: None,
            running: false,
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
            input: Default::default(),
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
