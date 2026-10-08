use std::collections::BTreeSet;

use crate::launch::Repo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Heading(&'static str),
    Recent(usize),
    Org {
        name: String,
        count: usize,
        open: bool,
    },
    Repo(usize),
    Create {
        org: String,
        name: String,
    },
}

impl Item {
    fn selectable(&self) -> bool {
        !matches!(self, Self::Heading(_))
    }
}

pub struct Picker {
    pub filter: String,
    pub cursor: usize,
    open: BTreeSet<String>,
    recent: Vec<usize>,
}

impl Picker {
    pub fn new(recent: Vec<usize>, repos: &[Repo]) -> Self {
        let mut picker = Self {
            filter: String::new(),
            cursor: 0,
            open: BTreeSet::new(),
            recent,
        };
        picker.reset_cursor(repos);
        picker
    }

    pub fn filtering(&self) -> bool {
        !self.filter.trim().is_empty()
    }

    pub fn items(&self, repos: &[Repo]) -> Vec<Item> {
        if self.filtering() {
            let filter = self.filter.trim().to_lowercase();
            let mut items: Vec<Item> = repos
                .iter()
                .enumerate()
                .filter(|(_, repo)| {
                    repo.label().to_lowercase().contains(&filter)
                        || repo.folder_label().to_lowercase().contains(&filter)
                })
                .map(|(i, _)| Item::Repo(i))
                .collect();
            items.extend(self.create_options(repos));
            return items;
        }
        let mut items = Vec::new();
        if !self.recent.is_empty() {
            items.push(Item::Heading("recent"));
            items.extend(self.recent.iter().map(|&i| Item::Recent(i)));
        }
        items.push(Item::Heading("all projects"));
        for org in orgs(repos) {
            let members: Vec<usize> = (0..repos.len()).filter(|&i| repos[i].org == org).collect();
            let open = self.open.contains(&org);
            items.push(Item::Org {
                name: org,
                count: members.len(),
                open,
            });
            if open {
                items.extend(members.into_iter().map(Item::Repo));
            }
        }
        items
    }

    fn create_options(&self, repos: &[Repo]) -> Vec<Item> {
        let filter = self.filter.trim();
        let folder_orgs: BTreeSet<String> = repos.iter().map(|r| r.folder_org.clone()).collect();
        let (orgs, name): (Vec<String>, &str) = match filter.split_once('/') {
            Some((org, name)) => (folder_orgs.into_iter().filter(|o| o == org).collect(), name),
            None => (folder_orgs.into_iter().collect(), filter),
        };
        if !valid_name(name) {
            return Vec::new();
        }
        orgs.into_iter()
            .filter(|org| {
                !repos
                    .iter()
                    .any(|r| &r.folder_org == org && r.folder == name)
            })
            .map(|org| Item::Create {
                org,
                name: name.to_string(),
            })
            .collect()
    }

    pub fn current(&self, repos: &[Repo]) -> Option<Item> {
        self.items(repos)
            .get(self.cursor)
            .cloned()
            .filter(Item::selectable)
    }

    pub fn reset_cursor(&mut self, repos: &[Repo]) {
        self.cursor = self
            .items(repos)
            .iter()
            .position(Item::selectable)
            .unwrap_or(0);
    }

    pub fn step(&mut self, repos: &[Repo], direction: isize) {
        let items = self.items(repos);
        let next = if direction > 0 {
            (self.cursor + 1..items.len()).find(|&i| items[i].selectable())
        } else {
            (0..self.cursor).rev().find(|&i| items[i].selectable())
        };
        if let Some(i) = next {
            self.cursor = i;
        }
    }

    pub fn open_current(&mut self, repos: &[Repo]) {
        if let Some(Item::Org { name, .. }) = self.current(repos) {
            self.open.insert(name);
        }
    }

    pub fn close_current(&mut self, repos: &[Repo]) {
        let items = self.items(repos);
        let org = match items.get(self.cursor) {
            Some(Item::Org { name, .. }) => name.clone(),
            Some(Item::Repo(i)) if !self.filtering() => repos[*i].org.clone(),
            _ => return,
        };
        self.open.remove(&org);
        self.cursor = self
            .items(repos)
            .iter()
            .position(|item| matches!(item, Item::Org { name, .. } if *name == org))
            .unwrap_or(0);
    }

    pub fn toggle_current(&mut self, repos: &[Repo]) {
        match self.current(repos) {
            Some(Item::Org { open: true, .. }) => self.close_current(repos),
            Some(Item::Org { .. }) => self.open_current(repos),
            _ => {}
        }
    }

    pub fn type_char(&mut self, repos: &[Repo], c: char) {
        self.filter.push(c);
        self.reset_cursor(repos);
    }

    pub fn set_filter(&mut self, repos: &[Repo], filter: String) {
        self.filter = filter;
        self.reset_cursor(repos);
    }

    pub fn backspace(&mut self, repos: &[Repo]) {
        self.filter.pop();
        self.reset_cursor(repos);
    }
}

fn orgs(repos: &[Repo]) -> BTreeSet<String> {
    repos.iter().map(|r| r.org.clone()).collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn repos() -> Vec<Repo> {
        [
            "acme/billing",
            "acme/admin",
            "globex/engine",
            "ingarabr/ratatosk",
        ]
        .iter()
        .map(|label| {
            let (org, name) = label.split_once('/').unwrap();
            Repo {
                org: org.into(),
                name: name.into(),
                dir: PathBuf::from("/p").join(label),
                kind: crate::place::Kind::Folder,
                folder_org: org.into(),
                folder: name.into(),
            }
        })
        .collect()
    }

    #[test]
    fn starts_with_recent_then_collapsed_orgs() {
        let repos = repos();
        let picker = Picker::new(vec![3], &repos);
        let items = picker.items(&repos);
        assert_eq!(items[0], Item::Heading("recent"));
        assert_eq!(items[1], Item::Recent(3));
        assert_eq!(items[2], Item::Heading("all projects"));
        assert_eq!(
            items[3],
            Item::Org {
                name: "acme".into(),
                count: 2,
                open: false
            }
        );
        assert_eq!(items.len(), 6);
        assert_eq!(picker.cursor, 1);
    }

    #[test]
    fn opening_an_org_shows_its_repos_and_closing_from_a_repo_returns_to_it() {
        let repos = repos();
        let mut picker = Picker::new(Vec::new(), &repos);
        picker.open_current(&repos);
        picker.step(&repos, 1);
        assert_eq!(picker.current(&repos), Some(Item::Repo(0)));
        picker.close_current(&repos);
        assert_eq!(
            picker.current(&repos),
            Some(Item::Org {
                name: "acme".into(),
                count: 2,
                open: false
            })
        );
    }

    #[test]
    fn filtering_lists_matches_then_new_folder_options() {
        let repos = repos();
        let mut picker = Picker::new(Vec::new(), &repos);
        for c in "ac".chars() {
            picker.type_char(&repos, c);
        }
        let items = picker.items(&repos);
        assert_eq!(items[..2], [Item::Repo(0), Item::Repo(1)]);
        assert!(items.contains(&Item::Create {
            org: "ingarabr".into(),
            name: "ac".into()
        }));
    }

    #[test]
    fn no_new_folder_option_where_the_repo_already_exists() {
        let repos = repos();
        let mut picker = Picker::new(Vec::new(), &repos);
        for c in "billing".chars() {
            picker.type_char(&repos, c);
        }
        let items = picker.items(&repos);
        assert_eq!(items[0], Item::Repo(0));
        assert!(!items.contains(&Item::Create {
            org: "acme".into(),
            name: "billing".into()
        }));
        assert!(items.contains(&Item::Create {
            org: "globex".into(),
            name: "billing".into()
        }));
    }

    #[test]
    fn org_slash_name_offers_only_that_org() {
        let repos = repos();
        let mut picker = Picker::new(Vec::new(), &repos);
        for c in "ingarabr/new-tool".chars() {
            picker.type_char(&repos, c);
        }
        assert_eq!(
            picker.items(&repos),
            [Item::Create {
                org: "ingarabr".into(),
                name: "new-tool".into()
            }]
        );
    }
}
