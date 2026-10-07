use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::TableState,
};

use crate::{
    agents::{self, Session, State},
    config::ManualModel,
    git,
    launch::{self, Draft, Field, Launch, Repo, Target},
    names,
    place::{Place, Scope},
    pr::{self, Pr, PrState, RepoPrs},
    recent,
};

const RECENT_SHOWN: usize = 6;
const DELETE_WINDOW: Duration = Duration::from_secs(2);
const PR_MIN_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    List,
    Menu,
}

pub enum Line {
    Header(String),
    Session(usize),
}

pub enum Action {
    None,
    Quit,
    Attach { id: String, cwd: PathBuf },
    Start { launch: Launch, open: bool },
    Stop { id: String },
    Delete { id: String },
}

#[derive(Default)]
pub struct App {
    pub base: PathBuf,
    pub sessions: Vec<Session>,
    pub places: Vec<Place>,
    pub branches: Vec<Option<String>>,
    pub lines: Vec<Line>,
    pub table: TableState,
    pub error: Option<String>,
    pub repos: Vec<Repo>,
    pub input: String,
    pub draft: Option<Draft>,
    pub recent_path: Option<PathBuf>,
    pub status: Option<String>,
    pub armed_delete: Option<(String, Instant)>,
    pub focus: Focus,
    pub scopes: Vec<Scope>,
    pub scope: Scope,
    pub manual_model: ManualModel,
    pub names_path: Option<PathBuf>,
    pub names: HashMap<String, String>,
    pub claude_names: HashMap<String, String>,
    pub renaming: Option<(String, String)>,
    pub prs: HashMap<String, RepoPrs>,
    pub dir_repos: HashMap<PathBuf, String>,
    pub pr_lookup: Option<Receiver<pr::Found>>,
    pub prs_requested: Option<Instant>,
}

impl App {
    pub fn new(base: PathBuf, manual_model: ManualModel) -> Self {
        let mut app = Self {
            repos: launch::discover(&base),
            base,
            sessions: Vec::new(),
            places: Vec::new(),
            branches: Vec::new(),
            lines: Vec::new(),
            table: TableState::default(),
            error: None,
            input: String::new(),
            draft: None,
            recent_path: recent::path(),
            status: None,
            armed_delete: None,
            focus: Focus::List,
            scopes: vec![Scope::All],
            scope: Scope::All,
            manual_model,
            names: names::path().map(|p| names::load(&p)).unwrap_or_default(),
            names_path: names::path(),
            claude_names: HashMap::new(),
            renaming: None,
            prs: HashMap::new(),
            dir_repos: HashMap::new(),
            pr_lookup: None,
            prs_requested: None,
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        let keep = self.selected_session().map(|s| s.id.clone());
        match agents::load() {
            Ok(mut sessions) => {
                self.claude_names.clear();
                for session in &mut sessions {
                    if let Some(name) = self.names.get(&session.id) {
                        let claude = std::mem::replace(&mut session.name, name.clone());
                        self.claude_names.insert(session.id.clone(), claude);
                    }
                }
                self.sessions = sessions;
                self.error = None;
            }
            Err(err) => self.error = Some(format!("{err:#}")),
        }
        self.rebuild(keep.as_deref());
    }

    pub fn count(&self, state: State) -> usize {
        self.sessions.iter().filter(|s| s.state == state).count()
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.selected_index().and_then(|i| self.sessions.get(i))
    }

    pub fn select_started(&mut self, stdout: &str) -> Option<(String, PathBuf)> {
        let line = self.lines.iter().position(
            |line| matches!(line, Line::Session(i) if stdout.contains(&self.sessions[*i].id)),
        )?;
        self.table.select(Some(line));
        self.selected_session()
            .map(|s| (s.id.clone(), s.cwd.clone()))
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        self.status = None;
        if self.renaming.is_some() {
            self.on_rename_key(key);
            return Action::None;
        }
        if self.draft.is_some() {
            return self.on_draft_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.focus == Focus::Menu {
            match key.code {
                KeyCode::Up => return self.step_scope(-1),
                KeyCode::Down => return self.step_scope(1),
                KeyCode::Right | KeyCode::Enter | KeyCode::Esc => {
                    self.focus = Focus::List;
                    return Action::None;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Left if self.input.is_empty() => self.focus = Focus::Menu,
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('x') if ctrl => return self.stop_or_delete(Instant::now()),
            KeyCode::Char('r') if ctrl => {
                if let Some(s) = self.selected_session() {
                    self.renaming = Some((s.id.clone(), s.name.clone()));
                }
            }
            KeyCode::Esc if self.input.is_empty() => return Action::Quit,
            KeyCode::Esc => self.input.clear(),
            KeyCode::Enter if !self.input.trim().is_empty() => {
                let target = self
                    .selected_repo()
                    .or_else(|| self.scope_repo())
                    .map(Target::Repo);
                let recent = self.recent_repos();
                self.draft = Some(Draft::new(
                    self.input.trim().to_string(),
                    target,
                    recent,
                    &self.repos,
                    self.manual_model.clone(),
                ));
            }
            KeyCode::Enter => return self.attach_selected(),
            KeyCode::Right if self.input.is_empty() => return self.attach_selected(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) if !ctrl => self.input.push(c),
            KeyCode::Up => self.step(-1, 1),
            KeyCode::Down => self.step(1, 1),
            KeyCode::PageUp => self.step(-1, 10),
            KeyCode::PageDown => self.step(1, 10),
            KeyCode::Home => self.table.select(self.first_session_line()),
            KeyCode::End => {
                let last = self.session_lines().last();
                self.table.select(last);
            }
            _ => {}
        }
        Action::None
    }

    fn on_draft_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let (base, repos) = (&self.base, &self.repos);
        let Some(draft) = self.draft.as_mut() else {
            return Action::None;
        };
        let start = |draft: &mut Draft, open: bool| match draft.launch(base, repos) {
            Some(launch) => Action::Start { launch, open },
            None => {
                draft.notice = Some("pick a project, model and effort first".to_string());
                Action::None
            }
        };
        match key.code {
            KeyCode::Esc => self.draft = None,
            KeyCode::Char('c') if ctrl => self.draft = None,
            KeyCode::Enter if ctrl => return start(draft, true),
            KeyCode::Tab if shift => draft.field = draft.field.prev(),
            KeyCode::BackTab => draft.field = draft.field.prev(),
            KeyCode::Tab => draft.field = draft.field.next(),
            _ if draft.field == Field::Project => match key.code {
                KeyCode::Enter => draft.pick_current(repos),
                KeyCode::Up => draft.picker.step(repos, -1),
                KeyCode::Down => draft.picker.step(repos, 1),
                KeyCode::Right => draft.picker.open_current(repos),
                KeyCode::Left => draft.picker.close_current(repos),
                KeyCode::Backspace => draft.picker.backspace(repos),
                KeyCode::Char(c) if !ctrl => draft.picker.type_char(repos, c),
                _ => {}
            },
            KeyCode::Enter => return start(draft, false),
            KeyCode::Up => draft.field = draft.field.prev(),
            KeyCode::Down => draft.field = draft.field.next(),
            KeyCode::Left => draft.cycle(-1),
            KeyCode::Right => draft.cycle(1),
            _ => {}
        }
        Action::None
    }

    pub fn remember(&self, label: &str) {
        if let Some(path) = &self.recent_path {
            let _ = recent::remember(path, label);
        }
    }

    fn recent_repos(&self) -> Vec<usize> {
        let remembered = self
            .recent_path
            .as_deref()
            .map(recent::load)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|label| self.repos.iter().position(|r| r.label() == label));
        let mut by_session: Vec<usize> = (0..self.sessions.len()).collect();
        by_session.sort_by_key(|&i| std::cmp::Reverse(self.sessions[i].started_at_ms));
        let from_sessions = by_session
            .into_iter()
            .filter_map(|i| match &self.places[i] {
                Place::Repo { org, repo, .. } => self
                    .repos
                    .iter()
                    .position(|r| &r.org == org && &r.name == repo),
                Place::Outside => None,
            });
        let mut recent = Vec::new();
        for i in remembered.chain(from_sessions) {
            if !recent.contains(&i) {
                recent.push(i);
            }
        }
        recent.truncate(RECENT_SHOWN);
        recent
    }

    fn on_rename_key(&mut self, key: KeyEvent) {
        let Some((id, buffer)) = self.renaming.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.renaming = None,
            KeyCode::Enter => {
                let (id, name) = (id.clone(), buffer.trim().to_string());
                self.renaming = None;
                self.rename(&id, &name);
            }
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buffer.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => buffer.push(c),
            _ => {}
        }
    }

    fn rename(&mut self, id: &str, name: &str) {
        let Some(i) = self.sessions.iter().position(|s| s.id == id) else {
            return;
        };
        let claude = self
            .claude_names
            .remove(id)
            .unwrap_or_else(|| self.sessions[i].name.clone());
        if name.is_empty() || name == claude {
            self.names.remove(id);
            self.sessions[i].name = claude;
        } else {
            self.names.insert(id.to_string(), name.to_string());
            self.claude_names.insert(id.to_string(), claude);
            self.sessions[i].name = name.to_string();
        }
        if let Some(path) = &self.names_path
            && let Err(err) = names::save(path, &self.names)
        {
            self.error = Some(format!("could not save names: {err}"));
        }
        self.layout(Some(id));
    }

    pub fn repo_dir(&self, i: usize) -> Option<PathBuf> {
        match &self.places[i] {
            Place::Repo { org, repo, .. } => Some(self.base.join(org).join(repo)),
            Place::Outside => None,
        }
    }

    pub fn prs_for(&self, i: usize) -> Vec<Pr> {
        let known = |repo: &str, number: u64| {
            self.prs
                .get(&repo.to_lowercase())
                .and_then(|p| p.by_number.get(&number))
                .cloned()
        };
        let mut prs: Vec<Pr> = self.sessions[i]
            .prs
            .iter()
            .map(|r| {
                known(&r.repo, r.number).unwrap_or(Pr {
                    number: r.number,
                    state: PrState::Unknown,
                    title: String::new(),
                    url: format!("https://github.com/{}/pull/{}", r.repo, r.number),
                })
            })
            .collect();
        if prs.is_empty()
            && let (Some(dir), Some(Some(branch))) = (self.repo_dir(i), self.branches.get(i))
            && let Some(repo) = self.dir_repos.get(&dir).and_then(|name| self.prs.get(name))
            && let Some(pr) = repo
                .by_branch
                .get(branch)
                .and_then(|n| repo.by_number.get(n))
        {
            prs.push(pr.clone());
        }
        prs.dedup_by_key(|pr| pr.number);
        prs
    }

    pub fn request_prs(&mut self, now: Instant) {
        let recent = self
            .prs_requested
            .is_some_and(|at| now.duration_since(at) < PR_MIN_INTERVAL);
        if self.pr_lookup.is_some() || recent {
            return;
        }
        let mut repos: Vec<PathBuf> = (0..self.sessions.len())
            .filter_map(|i| self.repo_dir(i))
            .collect();
        repos.sort();
        repos.dedup();
        if repos.is_empty() && self.sessions.iter().all(|s| s.prs.is_empty()) {
            return;
        }
        let linked: Vec<(String, u64)> = self
            .sessions
            .iter()
            .flat_map(|s| s.prs.iter().map(|r| (r.repo.clone(), r.number)))
            .collect();
        self.prs_requested = Some(now);
        self.pr_lookup = Some(pr::look_up(repos, linked));
    }

    pub fn receive_prs(&mut self) -> bool {
        let Some(rx) = &self.pr_lookup else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok((dir, Ok(prs))) => {
                    let name = prs.repo.to_lowercase();
                    if let Some(dir) = dir {
                        self.dir_repos.insert(dir, name.clone());
                    }
                    self.prs.entry(name).or_default().merge(prs);
                    changed = true;
                }
                Ok((_, Err(_))) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pr_lookup = None;
                    break;
                }
            }
        }
        changed
    }

    pub fn prs_pending(&self) -> bool {
        self.pr_lookup.is_some()
    }

    pub fn stop_or_delete(&mut self, now: Instant) -> Action {
        let Some(id) = self.selected_session().map(|s| s.id.clone()) else {
            return Action::None;
        };
        let armed = self
            .armed_delete
            .take()
            .is_some_and(|(armed, at)| armed == id && now.duration_since(at) <= DELETE_WINDOW);
        if armed {
            return Action::Delete { id };
        }
        self.armed_delete = Some((id.clone(), now));
        Action::Stop { id }
    }

    fn attach_selected(&self) -> Action {
        self.selected_session()
            .map_or(Action::None, |s| Action::Attach {
                id: s.id.clone(),
                cwd: s.cwd.clone(),
            })
    }

    pub fn selected_session_index(&self) -> Option<usize> {
        self.selected_index()
    }

    fn selected_index(&self) -> Option<usize> {
        match self.lines.get(self.table.selected()?) {
            Some(Line::Session(i)) => Some(*i),
            _ => None,
        }
    }

    fn selected_repo(&self) -> Option<usize> {
        match self.places.get(self.selected_index()?)? {
            Place::Repo { org, repo, .. } => self
                .repos
                .iter()
                .position(|r| &r.org == org && &r.name == repo),
            Place::Outside => None,
        }
    }

    fn rebuild(&mut self, keep: Option<&str>) {
        self.places = self
            .sessions
            .iter()
            .map(|s| Place::of(&self.base, &s.cwd))
            .collect();
        let mut by_dir: HashMap<PathBuf, Option<String>> = HashMap::new();
        self.branches = self
            .sessions
            .iter()
            .zip(&self.places)
            .map(|(session, place)| match place {
                Place::Repo { .. } => by_dir
                    .entry(session.cwd.clone())
                    .or_insert_with(|| git::branch(&session.cwd))
                    .clone(),
                Place::Outside => None,
            })
            .collect();
        self.scopes = Scope::for_places(&self.places);
        if !self.scopes.contains(&self.scope) {
            self.scope = Scope::All;
        }
        self.layout(keep);
    }

    fn layout(&mut self, keep: Option<&str>) {
        let places = &self.places;
        let mut order: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| self.scope.matches(&places[i]))
            .collect();
        order.sort_by_key(|&i| {
            let place = &places[i];
            (
                matches!(place, Place::Outside),
                place.group(),
                self.sessions[i].name.to_lowercase(),
            )
        });

        let mut lines = Vec::new();
        let mut current = None;
        for i in order {
            let group = places[i].group();
            if current.as_ref() != Some(&group) {
                lines.push(Line::Header(group.clone()));
                current = Some(group);
            }
            lines.push(Line::Session(i));
        }
        self.lines = lines;

        let kept = keep.and_then(|id| {
            self.lines
                .iter()
                .position(|line| matches!(line, Line::Session(i) if self.sessions[*i].id == id))
        });
        let first = self.first_session_line();
        self.table.select(kept.or(first));
    }

    fn step_scope(&mut self, direction: isize) -> Action {
        let at = self
            .scopes
            .iter()
            .position(|s| *s == self.scope)
            .unwrap_or(0) as isize;
        let next = (at + direction).clamp(0, self.scopes.len() as isize - 1) as usize;
        self.scope = self.scopes[next].clone();
        let keep = self.selected_session().map(|s| s.id.clone());
        self.layout(keep.as_deref());
        Action::None
    }

    pub fn scope_count(&self, scope: &Scope) -> usize {
        self.places.iter().filter(|p| scope.matches(p)).count()
    }

    fn scope_repo(&self) -> Option<usize> {
        match &self.scope {
            Scope::Repo { org, repo } => self
                .repos
                .iter()
                .position(|r| &r.org == org && &r.name == repo),
            _ => None,
        }
    }

    fn first_session_line(&self) -> Option<usize> {
        self.session_lines().next()
    }

    fn session_lines(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, Line::Session(_)))
            .map(|(i, _)| i)
    }

    fn step(&mut self, direction: isize, times: usize) {
        let Some(mut at) = self.table.selected() else {
            return self.table.select(self.first_session_line());
        };
        for _ in 0..times {
            let next = if direction > 0 {
                self.session_lines().find(|&i| i > at)
            } else {
                self.session_lines().rev().find(|&i| i < at)
            };
            match next {
                Some(i) => at = i,
                None => break,
            }
        }
        self.table.select(Some(at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(ids: &[&str]) -> App {
        let sessions: Vec<Session> = ids
            .iter()
            .map(|id| Session {
                id: id.to_string(),
                name: id.to_string(),
                state: State::Done,
                cwd: PathBuf::from("/elsewhere"),
                started_at_ms: None,
                detail: None,
                prs: Vec::new(),
            })
            .collect();
        App {
            base: PathBuf::from("/p"),
            places: vec![Place::Outside; sessions.len()],
            branches: vec![None; sessions.len()],
            lines: (0..sessions.len()).map(Line::Session).collect(),
            sessions,
            table: TableState::default().with_selected(Some(0)),
            error: None,
            repos: Vec::new(),
            input: String::new(),
            draft: None,
            recent_path: None,
            status: None,
            armed_delete: None,
            focus: Focus::List,
            scopes: vec![Scope::All],
            scope: Scope::All,
            manual_model: ManualModel::default(),
            ..Default::default()
        }
    }

    #[test]
    fn ctrl_x_stops_and_a_second_press_soon_after_deletes() {
        let mut app = app_with(&["a", "b"]);
        let t0 = Instant::now();
        assert!(matches!(app.stop_or_delete(t0), Action::Stop { id } if id == "a"));
        assert!(
            matches!(app.stop_or_delete(t0 + Duration::from_secs(1)), Action::Delete { id } if id == "a")
        );
        assert!(matches!(
            app.stop_or_delete(t0 + Duration::from_secs(2)),
            Action::Stop { .. }
        ));
    }

    #[test]
    fn a_late_or_moved_second_press_only_stops() {
        let mut app = app_with(&["a", "b"]);
        let t0 = Instant::now();
        app.stop_or_delete(t0);
        assert!(matches!(
            app.stop_or_delete(t0 + Duration::from_secs(3)),
            Action::Stop { .. }
        ));
        app.table.select(Some(1));
        assert!(
            matches!(app.stop_or_delete(t0 + Duration::from_secs(3)), Action::Stop { id } if id == "b")
        );
    }
}

#[cfg(test)]
mod scope_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    #[test]
    fn left_opens_the_menu_and_moving_scope_filters_the_list() {
        let mut app = App {
            base: PathBuf::from("/p"),
            sessions: Vec::new(),
            places: Vec::new(),
            branches: Vec::new(),
            lines: Vec::new(),
            table: TableState::default(),
            error: None,
            repos: Vec::new(),
            input: String::new(),
            draft: None,
            recent_path: None,
            status: None,
            armed_delete: None,
            manual_model: Default::default(),
            focus: Focus::List,
            scopes: vec![Scope::All],
            scope: Scope::All,
            ..Default::default()
        };
        for (id, cwd) in [
            ("a", "/p/acme/billing"),
            ("b", "/p/globex/engine"),
            ("c", "/elsewhere"),
        ] {
            app.sessions.push(Session {
                id: id.into(),
                name: id.into(),
                state: State::Done,
                cwd: PathBuf::from(cwd),
                started_at_ms: None,
                detail: None,
                prs: Vec::new(),
            });
        }
        app.rebuild(None);
        app.on_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.focus, Focus::Menu);
        app.on_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.scope, Scope::Org("acme".into()));
        let shown: Vec<&str> = app
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Session(i) => Some(app.sessions[*i].id.as_str()),
                Line::Header(_) => None,
            })
            .collect();
        assert_eq!(shown, ["a"]);
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.focus, Focus::List);
    }
}

#[cfg(test)]
mod rename_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    fn app() -> App {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        app.sessions.push(Session {
            id: "a1".into(),
            name: "billing: credit-note".into(),
            state: State::Done,
            cwd: PathBuf::from("/elsewhere"),
            started_at_ms: None,
            detail: None,
            prs: Vec::new(),
        });
        app.rebuild(None);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::from(code));
    }

    fn ctrl(app: &mut App, c: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn ctrl_r_edits_the_name_in_place_and_enter_keeps_it() {
        let mut app = app();
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        type_text(&mut app, "credit notes");
        assert!(app.input.is_empty());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.sessions[0].name, "credit notes");
        assert_eq!(app.names["a1"], "credit notes");
        assert_eq!(app.claude_names["a1"], "billing: credit-note");
    }

    #[test]
    fn esc_cancels_and_an_empty_name_restores_claudes() {
        let mut app = app();
        ctrl(&mut app, 'r');
        type_text(&mut app, " x");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.sessions[0].name, "billing: credit-note");

        app.rename("a1", "credit notes");
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'u');
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.sessions[0].name, "billing: credit-note");
        assert!(app.names.is_empty());
    }
}

#[cfg(test)]
mod pr_tests {
    use super::*;
    use crate::agents::PrRef;

    fn app_with_repo_prs() -> App {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        app.sessions.push(Session {
            id: "a1".into(),
            name: "refunds".into(),
            state: State::Done,
            cwd: PathBuf::from("/p/acme/billing/.claude/worktrees/refunds"),
            started_at_ms: None,
            detail: None,
            prs: Vec::new(),
        });
        app.places = vec![Place::of(&app.base, &app.sessions[0].cwd)];
        app.branches = vec![Some("refunds".into())];
        let mut repo = RepoPrs {
            repo: "acme/billing".into(),
            ..Default::default()
        };
        repo.by_branch.insert("refunds".into(), 11);
        repo.by_number.insert(
            11,
            Pr {
                number: 11,
                state: PrState::Merged,
                title: "Refunds".into(),
                url: String::new(),
            },
        );
        repo.by_number.insert(
            12,
            Pr {
                number: 12,
                state: PrState::Open,
                title: "Payouts".into(),
                url: String::new(),
            },
        );
        app.dir_repos
            .insert(PathBuf::from("/p/acme/billing"), "acme/billing".into());
        app.prs.insert("acme/billing".into(), repo);
        app
    }

    #[test]
    fn without_links_a_session_finds_its_pr_by_branch() {
        let app = app_with_repo_prs();
        assert_eq!(
            app.prs_for(0).iter().map(|p| p.number).collect::<Vec<_>>(),
            [11]
        );
    }

    #[test]
    fn linked_prs_win_and_unknown_ones_keep_their_number() {
        let mut app = app_with_repo_prs();
        app.sessions[0].prs = vec![
            PrRef {
                repo: "acme/billing".into(),
                number: 12,
            },
            PrRef {
                repo: "acme/billing".into(),
                number: 99,
            },
        ];
        let prs = app.prs_for(0);
        assert_eq!(
            prs.iter().map(|p| (p.number, p.state)).collect::<Vec<_>>(),
            [(12, PrState::Open), (99, PrState::Unknown)]
        );
    }
}
