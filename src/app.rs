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
    open::{self, Opener},
    place::{Place, Scope},
    pr::{self, Pr, PrState, RepoPrs},
    recent,
    state::{Sort, ViewState},
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
    Open { opener: usize, dir: PathBuf },
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
    pub view: ViewState,
    pub view_path: Option<PathBuf>,
    pub openers: Vec<Opener>,
    pub fitting: Vec<Vec<usize>>,
    pub choosing: Option<Choice>,
}

pub struct Choice {
    pub dir: PathBuf,
    pub options: Vec<usize>,
    pub cursor: usize,
}

impl App {
    pub fn new(base: PathBuf, manual_model: ManualModel, openers: Vec<Opener>) -> Self {
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
            view: ViewState::path()
                .map(|p| ViewState::load(&p))
                .unwrap_or_default(),
            view_path: ViewState::path(),
            openers,
            fitting: Vec::new(),
            choosing: None,
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
        if self.choosing.is_some() {
            return self.on_choice_key(key);
        }
        if self.draft.is_some() {
            return self.on_draft_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.focus = match self.focus {
                Focus::List => Focus::Menu,
                Focus::Menu => Focus::List,
            };
            return Action::None;
        }
        if self.focus == Focus::Menu {
            match key.code {
                KeyCode::Up => return self.step_scope(-1),
                KeyCode::Down => return self.step_scope(1),
                KeyCode::Left if self.input.is_empty() => {
                    self.menu_left();
                    return Action::None;
                }
                KeyCode::Right if self.input.is_empty() => {
                    self.menu_right();
                    return Action::None;
                }
                KeyCode::Enter | KeyCode::Esc => {
                    self.focus = Focus::List;
                    return Action::None;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Left if self.input.is_empty() => self.list_left(),
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('x') if ctrl => return self.stop_or_delete(Instant::now()),
            KeyCode::Char('o') if ctrl => return self.open_selected(),
            KeyCode::Char('s') if ctrl => {
                self.view.sort = self.view.sort.next();
                self.save_view();
                let keep = self.selected_session().map(|s| s.id.clone());
                self.layout(keep.as_deref());
            }
            KeyCode::Char('r') if ctrl => {
                if let Some(s) = self.selected_session() {
                    self.renaming = Some((s.id.clone(), s.name.clone()));
                }
            }
            KeyCode::Esc if self.input.is_empty() => return Action::Quit,
            KeyCode::Esc => self.input.clear(),
            KeyCode::Enter if !self.input.trim().is_empty() => self.open_draft(),
            KeyCode::Enter if self.selected_index().is_none() => self.toggle_group(),
            KeyCode::Enter => return self.attach_selected(),
            KeyCode::Right if self.input.is_empty() => return self.list_right(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) if !ctrl => self.input.push(c),
            KeyCode::Up => self.step(-1, 1),
            KeyCode::Down => self.step(1, 1),
            KeyCode::PageUp => self.step(-1, 10),
            KeyCode::PageDown => self.step(1, 10),
            KeyCode::Home if !self.lines.is_empty() => self.table.select(Some(0)),
            KeyCode::End if !self.lines.is_empty() => self.table.select(Some(self.lines.len() - 1)),
            _ => {}
        }
        Action::None
    }

    fn open_draft(&mut self) {
        let (target, org) = match &self.scope {
            Scope::Repo { .. } => (self.scope_repo().or_else(|| self.selected_repo()), None),
            Scope::Org(org) => (None, Some(org.clone())),
            Scope::All | Scope::Outside => (self.selected_repo(), None),
        };
        let mut draft = Draft::new(
            self.input.trim().to_string(),
            target.map(Target::Repo),
            self.recent_repos(),
            &self.repos,
            self.manual_model.clone(),
        );
        if let Some(org) = org {
            draft.picker.set_filter(&self.repos, format!("{org}/"));
        }
        self.draft = Some(draft);
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
        let mut by_root: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        self.fitting = (0..self.sessions.len())
            .map(|i| {
                let root = self.root_dir(i);
                by_root
                    .entry(root.clone())
                    .or_insert_with(|| open::fitting(&self.openers, &root))
                    .clone()
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
        let sort = self.view.sort;
        order.sort_by_key(|&i| {
            let place = &places[i];
            let session = &self.sessions[i];
            let recency = match sort {
                Sort::Active => std::cmp::Reverse(session.active_at_ms.unwrap_or(0)),
                Sort::Name => std::cmp::Reverse(0),
            };
            (
                matches!(place, Place::Outside),
                place.group(),
                recency,
                session.name.to_lowercase(),
            )
        });

        let mut lines = Vec::new();
        let mut current = None;
        for i in order {
            let group = places[i].group();
            if current.as_ref() != Some(&group) {
                lines.push(Line::Header(group.clone()));
                current = Some(group.clone());
            }
            if !self.view.collapsed_groups.contains(&group) {
                lines.push(Line::Session(i));
            }
        }
        self.lines = lines;

        let kept = keep.and_then(|id| {
            let session = self.sessions.iter().position(|s| s.id == id)?;
            self.lines
                .iter()
                .position(|line| matches!(line, Line::Session(i) if *i == session))
                .or_else(|| self.header_line(&self.places[session].group()))
        });
        let first = self
            .first_session_line()
            .or((!self.lines.is_empty()).then_some(0));
        self.table.select(kept.or(first));
    }

    fn header_line(&self, group: &str) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| matches!(line, Line::Header(g) if g == group))
    }

    fn selected_group(&self) -> Option<String> {
        match self.lines.get(self.table.selected()?)? {
            Line::Header(group) => Some(group.clone()),
            Line::Session(i) => Some(self.places[*i].group()),
        }
    }

    fn toggle_group(&mut self) {
        let Some(group) = self.selected_group() else {
            return;
        };
        if !self.view.collapsed_groups.remove(&group) {
            self.view.collapsed_groups.insert(group.clone());
        }
        self.save_view();
        self.layout(None);
        self.table.select(self.header_line(&group));
    }

    pub fn root_dir(&self, i: usize) -> PathBuf {
        match &self.places[i] {
            Place::Repo {
                org,
                repo,
                worktree: Some(name),
            } => self
                .base
                .join(org)
                .join(repo)
                .join(".claude/worktrees")
                .join(name),
            Place::Repo {
                org,
                repo,
                worktree: None,
            } => self.base.join(org).join(repo),
            Place::Outside => self.sessions[i].cwd.clone(),
        }
    }

    fn open_selected(&mut self) -> Action {
        let Some(i) = self.selected_index() else {
            return Action::None;
        };
        let dir = self.root_dir(i);
        match self.fitting.get(i).map_or(&[][..], Vec::as_slice) {
            [] => {
                self.status = Some(
                    "no opener fits this folder; add one under \"openers\" in the config"
                        .to_string(),
                );
                Action::None
            }
            [only] => Action::Open { opener: *only, dir },
            many => {
                self.choosing = Some(Choice {
                    dir,
                    options: many.to_vec(),
                    cursor: 0,
                });
                Action::None
            }
        }
    }

    fn on_choice_key(&mut self, key: KeyEvent) -> Action {
        let Some(choice) = self.choosing.as_mut() else {
            return Action::None;
        };
        let pick = |choice: &Choice, n: usize| Action::Open {
            opener: choice.options[n],
            dir: choice.dir.clone(),
        };
        let action = match key.code {
            KeyCode::Esc => Action::None,
            KeyCode::Up => {
                choice.cursor = choice.cursor.saturating_sub(1);
                return Action::None;
            }
            KeyCode::Down => {
                choice.cursor = (choice.cursor + 1).min(choice.options.len() - 1);
                return Action::None;
            }
            KeyCode::Enter => pick(choice, choice.cursor),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let n = c as usize - '1' as usize;
                if n >= choice.options.len() {
                    return Action::None;
                }
                pick(choice, n)
            }
            _ => return Action::None,
        };
        self.choosing = None;
        action
    }

    fn list_left(&mut self) {
        match self.lines.get(self.table.selected().unwrap_or(usize::MAX)) {
            Some(Line::Header(group)) if !self.view.collapsed_groups.contains(group) => {
                self.toggle_group()
            }
            _ => self.focus = Focus::Menu,
        }
    }

    fn list_right(&mut self) -> Action {
        let Some(at) = self.table.selected() else {
            return Action::None;
        };
        match self.lines.get(at) {
            Some(Line::Header(group)) if self.view.collapsed_groups.contains(group) => {
                self.toggle_group()
            }
            Some(Line::Header(_)) => {
                if matches!(self.lines.get(at + 1), Some(Line::Session(_))) {
                    self.table.select(Some(at + 1));
                }
            }
            Some(Line::Session(_)) => return self.attach_selected(),
            None => {}
        }
        Action::None
    }

    fn menu_left(&mut self) {
        match self.scope.clone() {
            Scope::Org(org) if !self.view.collapsed_orgs.contains(&org) => {
                self.view.collapsed_orgs.insert(org);
                self.save_view();
            }
            Scope::Repo { org, .. } => {
                self.scope = Scope::Org(org);
                let keep = self.selected_session().map(|s| s.id.clone());
                self.layout(keep.as_deref());
            }
            _ => {}
        }
    }

    fn menu_right(&mut self) {
        match self.scope.clone() {
            Scope::Org(org) if self.view.collapsed_orgs.contains(&org) => {
                self.view.collapsed_orgs.remove(&org);
                self.save_view();
            }
            _ => self.focus = Focus::List,
        }
    }

    fn save_view(&mut self) {
        if let Some(path) = &self.view_path
            && let Err(err) = self.view.save(path)
        {
            self.error = Some(format!("could not save the view settings: {err}"));
        }
    }

    pub fn visible_scopes(&self) -> Vec<&Scope> {
        self.scopes
            .iter()
            .filter(|scope| !matches!(scope, Scope::Repo { org, .. } if self.view.collapsed_orgs.contains(org)))
            .collect()
    }

    pub fn group_count(&self, group: &str) -> usize {
        (0..self.sessions.len())
            .filter(|&i| self.scope.matches(&self.places[i]) && self.places[i].group() == group)
            .count()
    }

    fn step_scope(&mut self, direction: isize) -> Action {
        let visible: Vec<Scope> = self.visible_scopes().into_iter().cloned().collect();
        let at = visible.iter().position(|s| *s == self.scope).unwrap_or(0) as isize;
        let next = (at + direction).clamp(0, visible.len() as isize - 1) as usize;
        self.scope = visible[next].clone();
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
        if self.lines.is_empty() {
            return;
        }
        let at = self.table.selected().unwrap_or(0) as isize;
        let next = (at + direction * times as isize).clamp(0, self.lines.len() as isize - 1);
        self.table.select(Some(next as usize));
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
                active_at_ms: None,
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
                active_at_ms: None,
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
            active_at_ms: None,
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
            active_at_ms: None,
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

#[cfg(test)]
mod draft_scope_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;
    use crate::launch::Field;

    fn app(scope: Scope) -> App {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        app.repos = ["acme/billing", "acme/admin", "globex/engine"]
            .iter()
            .map(|label| {
                let (org, name) = label.split_once('/').unwrap();
                Repo {
                    org: org.into(),
                    name: name.into(),
                    dir: PathBuf::from("/p").join(label),
                }
            })
            .collect();
        app.sessions.push(Session {
            id: "a1".into(),
            name: "engine work".into(),
            state: State::Done,
            cwd: PathBuf::from("/p/globex/engine"),
            started_at_ms: None,
            active_at_ms: None,
            detail: None,
            prs: Vec::new(),
        });
        app.rebuild(None);
        app.scope = scope;
        app.input = "start something".into();
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app
    }

    #[test]
    fn a_repo_filter_preselects_that_repo() {
        let app = app(Scope::Repo {
            org: "acme".into(),
            repo: "admin".into(),
        });
        assert_eq!(app.draft.unwrap().target, Some(Target::Repo(1)));
    }

    #[test]
    fn an_org_filter_opens_the_picker_on_that_org() {
        let app = app(Scope::Org("acme".into()));
        let draft = app.draft.unwrap();
        assert_eq!(draft.target, None);
        assert_eq!(draft.field, Field::Project);
        assert_eq!(draft.picker.filter, "acme/");
    }

    #[test]
    fn without_a_filter_the_selected_session_repo_is_used() {
        let app = app(Scope::All);
        assert_eq!(app.draft.unwrap().target, Some(Target::Repo(2)));
    }
}

#[cfg(test)]
mod collapse_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    fn app() -> App {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        for (id, cwd) in [
            ("a", "/p/acme/billing"),
            ("b", "/p/acme/billing"),
            ("c", "/p/acme/admin"),
            ("d", "/p/globex/engine"),
        ] {
            app.sessions.push(Session {
                id: id.into(),
                name: id.into(),
                state: State::Done,
                cwd: PathBuf::from(cwd),
                started_at_ms: None,
                active_at_ms: None,
                detail: None,
                prs: Vec::new(),
            });
        }
        app.rebuild(None);
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::from(code));
    }

    fn shown(app: &App) -> Vec<String> {
        app.lines
            .iter()
            .map(|line| match line {
                Line::Header(group) => format!("[{group}]"),
                Line::Session(i) => app.sessions[*i].id.clone(),
            })
            .collect()
    }

    #[test]
    fn left_collapses_a_group_header_and_right_expands_it() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!(
            shown(&app),
            [
                "[acme / admin]",
                "c",
                "[acme / billing]",
                "[globex / engine]",
                "d"
            ]
        );
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.focus, Focus::List);
        press(&mut app, KeyCode::Left);
        assert_eq!(
            app.focus,
            Focus::Menu,
            "left on a collapsed group goes on to the menu"
        );
        app.focus = Focus::List;
        press(&mut app, KeyCode::Right);
        assert!(shown(&app).contains(&"a".to_string()));
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.selected_session().map(|s| s.id.as_str()),
            Some("a"),
            "right on an open group enters it"
        );
    }

    #[test]
    fn left_on_a_session_opens_the_menu() {
        let mut app = app();
        press(&mut app, KeyCode::Left);
        assert_eq!(app.focus, Focus::Menu);
    }

    #[test]
    fn menu_arrows_collapse_and_expand_orgs() {
        let mut app = app();
        app.focus = Focus::Menu;
        app.scope = Scope::Repo {
            org: "acme".into(),
            repo: "billing".into(),
        };
        press(&mut app, KeyCode::Left);
        assert_eq!(
            app.scope,
            Scope::Org("acme".into()),
            "left on a repo goes to its org"
        );
        press(&mut app, KeyCode::Left);
        assert!(
            !app.visible_scopes()
                .iter()
                .any(|s| matches!(s, Scope::Repo { org, .. } if org == "acme"))
        );
        press(&mut app, KeyCode::Right);
        assert!(
            app.visible_scopes()
                .iter()
                .any(|s| matches!(s, Scope::Repo { org, .. } if org == "acme"))
        );
        assert_eq!(app.focus, Focus::Menu);
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.focus,
            Focus::List,
            "right on an open org goes to the list"
        );
    }

    #[test]
    fn tab_switches_between_the_menu_and_the_list() {
        let mut app = app();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Menu);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::List);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Focus::Menu);
    }

    #[test]
    fn space_types_into_the_prompt() {
        let mut app = app();
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.input, " ");
    }

    #[test]
    fn collapsed_state_round_trips() {
        let path = std::env::temp_dir()
            .join(format!("ratatosk-collapsed-{}", std::process::id()))
            .join("c.json");
        let mut collapsed = ViewState::default();
        collapsed.collapsed_groups.insert("acme / billing".into());
        collapsed.collapsed_orgs.insert("globex".into());
        collapsed.save(&path).unwrap();
        assert_eq!(ViewState::load(&path), collapsed);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

#[cfg(test)]
mod open_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    fn app(fitting: Vec<usize>) -> App {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        app.openers = ["IntelliJ", "RustRover", "Finder"]
            .iter()
            .map(|name| Opener {
                name: name.to_string(),
                command: vec!["true".into()],
                when: Vec::new(),
            })
            .collect();
        app.sessions.push(Session {
            id: "a".into(),
            name: "a".into(),
            state: State::Done,
            cwd: PathBuf::from("/p/acme/billing/.claude/worktrees/refunds/src"),
            started_at_ms: None,
            active_at_ms: None,
            detail: None,
            prs: Vec::new(),
        });
        app.rebuild(None);
        app.fitting = vec![fitting];
        app
    }

    fn ctrl_o(app: &mut App) -> Action {
        app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
    }

    #[test]
    fn one_fitting_opener_opens_the_worktree_root_directly() {
        let mut app = app(vec![1]);
        let Action::Open { opener, dir } = ctrl_o(&mut app) else {
            panic!("expected Open")
        };
        assert_eq!(opener, 1);
        assert_eq!(
            dir,
            PathBuf::from("/p/acme/billing/.claude/worktrees/refunds")
        );
    }

    #[test]
    fn several_fitting_openers_ask_which_one() {
        let mut app = app(vec![0, 2]);
        assert!(matches!(ctrl_o(&mut app), Action::None));
        assert_eq!(
            app.choosing.as_ref().map(|c| c.options.clone()),
            Some(vec![0, 2])
        );
        let Action::Open { opener, .. } = app.on_key(KeyEvent::from(KeyCode::Char('2'))) else {
            panic!("expected Open")
        };
        assert_eq!(opener, 2);
        assert!(app.choosing.is_none());
    }

    #[test]
    fn no_fitting_opener_says_so() {
        let mut app = app(Vec::new());
        assert!(matches!(ctrl_o(&mut app), Action::None));
        assert!(app.status.as_deref().unwrap_or("").contains("openers"));
    }
}

#[cfg(test)]
mod sort_tests {
    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    #[test]
    fn ctrl_s_sorts_by_last_activity_within_each_group() {
        let mut app = App {
            base: PathBuf::from("/p"),
            ..Default::default()
        };
        for (id, cwd, active) in [
            ("alpha", "/p/acme/billing", 1),
            ("beta", "/p/acme/billing", 3),
            ("gamma", "/p/globex/engine", 2),
        ] {
            app.sessions.push(Session {
                id: id.into(),
                name: id.into(),
                state: State::Done,
                cwd: PathBuf::from(cwd),
                started_at_ms: None,
                active_at_ms: Some(active),
                detail: None,
                prs: Vec::new(),
            });
        }
        app.rebuild(None);
        let order = |app: &App| -> Vec<String> {
            app.lines
                .iter()
                .filter_map(|l| match l {
                    Line::Session(i) => Some(app.sessions[*i].id.clone()),
                    Line::Header(_) => None,
                })
                .collect()
        };
        assert_eq!(order(&app), ["alpha", "beta", "gamma"]);
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.view.sort, Sort::Active);
        assert_eq!(
            order(&app),
            ["beta", "alpha", "gamma"],
            "newest first, groups kept"
        );
    }
}
