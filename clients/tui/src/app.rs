// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Client state and key handling, kept apart from drawing so it can be
//! tested without a terminal.

use anyhow::Result;
use pressbox_api::{Feed, Item, ItemPage, ItemQuery};
use ratatui::crossterm::event::KeyCode;

use crate::client::Client;

const PAGE: u32 = 100;

/// The calls the app makes, so tests can swap in a fake server.
pub trait Backend {
    fn feeds(&self) -> Result<Vec<Feed>>;
    fn items(&self, query: &ItemQuery) -> Result<ItemPage>;
    fn item(&self, id: i64) -> Result<Item>;
    fn set_read(&self, id: i64, value: bool) -> Result<()>;
    fn set_starred(&self, id: i64, value: bool) -> Result<()>;
    fn mark_feed_read(&self, feed: &str) -> Result<u32>;
    fn refresh(&self) -> Result<()>;
}

impl Backend for Client {
    fn feeds(&self) -> Result<Vec<Feed>> {
        Client::feeds(self)
    }
    fn items(&self, query: &ItemQuery) -> Result<ItemPage> {
        Client::items(self, query)
    }
    fn item(&self, id: i64) -> Result<Item> {
        Client::item(self, id)
    }
    fn set_read(&self, id: i64, value: bool) -> Result<()> {
        Client::set_read(self, id, value)
    }
    fn set_starred(&self, id: i64, value: bool) -> Result<()> {
        Client::set_starred(self, id, value)
    }
    fn mark_feed_read(&self, feed: &str) -> Result<u32> {
        Client::mark_feed_read(self, feed)
    }
    fn refresh(&self) -> Result<()> {
        Client::refresh(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Feeds,
    Items,
    Reader,
}

/// Rows above the real feeds in the feed list.
pub const ALL_ROW: usize = 0;
pub const STARRED_ROW: usize = 1;
pub const FIXED_ROWS: usize = 2;

pub struct Reader {
    pub item: Item,
    pub scroll: u16,
}

pub enum Action {
    None,
    Quit,
    Open(String),
}

pub struct App {
    pub feeds: Vec<Feed>,
    pub feed_row: usize,
    pub items: Vec<Item>,
    pub item_row: usize,
    /// Where the next page starts, if there is one.
    pub next_page: Option<NextPage>,
    pub reader: Option<Reader>,
    pub focus: Pane,
    pub unread_only: bool,
    pub status: String,
    /// The active search, if any. Searches ignore the unread filter.
    pub search: Option<String>,
    /// Text being typed after `/`, while the search prompt is open.
    pub editing: Option<String>,
}

/// Where the next page of items starts: a cursor, or an offset from a server
/// too old to give cursors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextPage {
    Cursor(String),
    Offset(u32),
}

impl NextPage {
    fn of(page: &ItemPage) -> Option<Self> {
        page.next_cursor
            .clone()
            .map(Self::Cursor)
            .or(page.next_offset.map(Self::Offset))
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            feeds: Vec::new(),
            feed_row: ALL_ROW,
            items: Vec::new(),
            item_row: 0,
            next_page: None,
            reader: None,
            focus: Pane::Feeds,
            unread_only: true,
            status: String::new(),
            search: None,
            editing: None,
        }
    }

    pub fn total_unread(&self) -> u32 {
        self.feeds.iter().map(|f| f.unread).sum()
    }

    pub fn feed_title<'a>(&'a self, id: &'a str) -> &'a str {
        self.feeds
            .iter()
            .find(|f| f.id == id)
            .map_or(id, |f| f.title.as_str())
    }

    fn selected_feed(&self) -> Option<&Feed> {
        self.feed_row
            .checked_sub(FIXED_ROWS)
            .and_then(|i| self.feeds.get(i))
    }

    fn query(&self, from: Option<&NextPage>) -> ItemQuery {
        let starred = self.feed_row == STARRED_ROW;
        ItemQuery {
            feed: self.selected_feed().map(|f| f.id.clone()),
            starred: starred.then_some(true),
            unread: (self.unread_only && !starred && self.search.is_none()).then_some(true),
            q: self.search.clone(),
            limit: Some(PAGE),
            cursor: match from {
                Some(NextPage::Cursor(c)) => Some(c.clone()),
                _ => None,
            },
            offset: match from {
                Some(NextPage::Offset(o)) => Some(*o),
                _ => None,
            },
            ..Default::default()
        }
    }

    /// Reloads feeds and the item list, keeping the selections in range.
    pub fn reload(&mut self, backend: &impl Backend) {
        let result = (|| -> Result<()> {
            self.feeds = backend.feeds()?;
            self.feed_row = self.feed_row.min(self.feeds.len() + FIXED_ROWS - 1);
            self.load_items(backend)
        })();
        self.report(result);
    }

    fn load_items(&mut self, backend: &impl Backend) -> Result<()> {
        let page = backend.items(&self.query(None))?;
        self.next_page = NextPage::of(&page);
        self.items = page.items;
        self.item_row = self.item_row.min(self.items.len().saturating_sub(1));
        Ok(())
    }

    fn load_more(&mut self, backend: &impl Backend) -> Result<()> {
        if let Some(from) = self.next_page.take() {
            let page = match backend.items(&self.query(Some(&from))) {
                Ok(page) => page,
                Err(e) => {
                    self.next_page = Some(from);
                    return Err(e);
                }
            };
            self.next_page = NextPage::of(&page);
            self.items.extend(page.items);
        }
        Ok(())
    }

    fn report(&mut self, result: Result<()>) {
        if let Err(e) = result {
            self.status = format!("error: {e:#}");
        }
    }

    /// Keys while the `/` search prompt is open.
    fn edit_search(&mut self, key: KeyCode, backend: &impl Backend) -> Result<()> {
        let Some(text) = self.editing.as_mut() else {
            return Ok(());
        };
        match key {
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Esc => self.editing = None,
            KeyCode::Enter => {
                let text = self.editing.take().unwrap_or_default();
                self.search = Some(text.trim().to_owned()).filter(|t| !t.is_empty());
                self.item_row = 0;
                self.reader = None;
                self.focus = Pane::Items;
                self.load_items(backend)?;
                if self.search.is_some() && self.items.is_empty() {
                    self.status = "no matches".into();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn handle(&mut self, key: KeyCode, backend: &impl Backend) -> Action {
        self.status.clear();
        if self.editing.is_some() {
            let result = self.edit_search(key, backend);
            self.report(result);
            return Action::None;
        }
        let mut action = Action::None;
        let result = (|| -> Result<()> {
            match key {
                KeyCode::Char('q') => action = Action::Quit,
                KeyCode::Char('/') => self.editing = Some(self.search.clone().unwrap_or_default()),
                KeyCode::Esc if self.search.is_some() && self.focus != Pane::Reader => {
                    self.search = None;
                    self.item_row = 0;
                    self.load_items(backend)?;
                }
                KeyCode::Esc => self.back(),
                KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => self.forward(backend)?,
                KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => self.back(),
                KeyCode::Char('j') | KeyCode::Down => self.step(1, backend)?,
                KeyCode::Char('k') | KeyCode::Up => self.step(-1, backend)?,
                KeyCode::PageDown | KeyCode::Char(' ') => self.step(10, backend)?,
                KeyCode::PageUp => self.step(-10, backend)?,
                KeyCode::Enter => self.enter(backend)?,
                KeyCode::Char('m') => self.toggle_read(backend)?,
                KeyCode::Char('s') => self.toggle_starred(backend)?,
                KeyCode::Char('o') => {
                    if let Some(url) = self.current_item().and_then(|i| i.url.clone()) {
                        self.mark(backend, true)?;
                        action = Action::Open(url);
                    }
                }
                KeyCode::Char('A') => self.mark_feed_read(backend)?,
                KeyCode::Char('u') => {
                    self.unread_only = !self.unread_only;
                    self.item_row = 0;
                    self.load_items(backend)?;
                    self.status = if self.unread_only {
                        "showing unread items".into()
                    } else {
                        "showing all items".into()
                    };
                }
                KeyCode::Char('r') => {
                    self.reload(backend);
                    if self.status.is_empty() {
                        self.status = "reloaded".into();
                    }
                }
                KeyCode::Char('R') => {
                    backend.refresh()?;
                    self.status = "server is refreshing feeds; press r in a moment".into();
                }
                _ => {}
            }
            Ok(())
        })();
        self.report(result);
        action
    }

    fn current_item(&self) -> Option<&Item> {
        match (&self.reader, self.focus) {
            (Some(r), Pane::Reader) => Some(&r.item),
            _ => self.items.get(self.item_row),
        }
    }

    fn back(&mut self) {
        self.focus = match self.focus {
            Pane::Reader => {
                self.reader = None;
                Pane::Items
            }
            Pane::Items | Pane::Feeds => Pane::Feeds,
        };
    }

    fn forward(&mut self, backend: &impl Backend) -> Result<()> {
        match self.focus {
            Pane::Feeds => self.focus = Pane::Items,
            Pane::Items => self.open_reader(backend)?,
            Pane::Reader => {}
        }
        Ok(())
    }

    fn enter(&mut self, backend: &impl Backend) -> Result<()> {
        match self.focus {
            Pane::Feeds => {
                self.focus = Pane::Items;
                Ok(())
            }
            Pane::Items => self.open_reader(backend),
            Pane::Reader => Ok(()),
        }
    }

    fn step(&mut self, delta: isize, backend: &impl Backend) -> Result<()> {
        match self.focus {
            Pane::Feeds => {
                let rows = self.feeds.len() + FIXED_ROWS;
                let row = self.feed_row.saturating_add_signed(delta).min(rows - 1);
                if row != self.feed_row {
                    self.feed_row = row;
                    self.item_row = 0;
                    self.load_items(backend)?;
                }
            }
            Pane::Items => {
                let last = self.items.len().saturating_sub(1);
                self.item_row = self.item_row.saturating_add_signed(delta).min(last);
                if self.item_row + 5 >= self.items.len() {
                    self.load_more(backend)?;
                }
            }
            Pane::Reader => {
                if let Some(r) = &mut self.reader {
                    let delta = i16::try_from(delta).unwrap_or(0);
                    r.scroll = r.scroll.saturating_add_signed(delta);
                }
            }
        }
        Ok(())
    }

    fn open_reader(&mut self, backend: &impl Backend) -> Result<()> {
        let Some(id) = self.items.get(self.item_row).map(|i| i.id) else {
            return Ok(());
        };
        let item = backend.item(id)?;
        self.reader = Some(Reader { item, scroll: 0 });
        self.focus = Pane::Reader;
        self.mark(backend, true)
    }

    /// Sets the read flag of the current item, locally and on the server.
    fn mark(&mut self, backend: &impl Backend, read: bool) -> Result<()> {
        let Some(item) = self.current_item() else {
            return Ok(());
        };
        if item.read == read {
            return Ok(());
        }
        let (id, feed_id) = (item.id, item.feed_id.clone());
        backend.set_read(id, read)?;
        self.update_item(id, |i| i.read = read);
        if let Some(feed) = self.feeds.iter_mut().find(|f| f.id == feed_id) {
            feed.unread = if read {
                feed.unread.saturating_sub(1)
            } else {
                feed.unread + 1
            };
        }
        Ok(())
    }

    fn toggle_read(&mut self, backend: &impl Backend) -> Result<()> {
        if let Some(read) = self.current_item().map(|i| i.read) {
            self.mark(backend, !read)?;
        }
        Ok(())
    }

    fn toggle_starred(&mut self, backend: &impl Backend) -> Result<()> {
        let Some((id, starred)) = self.current_item().map(|i| (i.id, i.starred)) else {
            return Ok(());
        };
        backend.set_starred(id, !starred)?;
        self.update_item(id, |i| i.starred = !starred);
        Ok(())
    }

    fn update_item(&mut self, id: i64, f: impl Fn(&mut Item)) {
        self.items.iter_mut().filter(|i| i.id == id).for_each(&f);
        if let Some(r) = self.reader.as_mut().filter(|r| r.item.id == id) {
            f(&mut r.item);
        }
    }

    fn mark_feed_read(&mut self, backend: &impl Backend) -> Result<()> {
        let ids: Vec<String> = match self.selected_feed() {
            Some(f) => vec![f.id.clone()],
            None if self.feed_row == ALL_ROW => self
                .feeds
                .iter()
                .filter(|f| f.unread > 0)
                .map(|f| f.id.clone())
                .collect(),
            None => return Ok(()),
        };
        let mut total = 0;
        for id in &ids {
            total += backend.mark_feed_read(id)?;
        }
        self.status = format!("marked {total} items read");
        self.reload(backend);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// An in-memory server with two feeds.
    struct Fake {
        items: RefCell<Vec<Item>>,
        calls: RefCell<Vec<String>>,
    }

    fn item(id: i64, feed: &str) -> Item {
        Item {
            id,
            feed_id: feed.into(),
            title: format!("item {id}"),
            url: Some(format!("https://example.com/{id}")),
            author: None,
            summary: None,
            content: None,
            published: 1000 - id,
            read: false,
            starred: false,
        }
    }

    impl Fake {
        fn new() -> Self {
            Self {
                items: RefCell::new(vec![item(1, "a"), item(2, "a"), item(3, "b")]),
                calls: RefCell::default(),
            }
        }
    }

    impl Backend for Fake {
        fn feeds(&self) -> Result<Vec<Feed>> {
            let items = self.items.borrow();
            Ok(["a", "b"]
                .iter()
                .map(|id| Feed {
                    id: (*id).into(),
                    title: id.to_uppercase(),
                    url: String::new(),
                    site_url: None,
                    tags: vec![],
                    unread: items
                        .iter()
                        .filter(|i| i.feed_id == *id && !i.read)
                        .count()
                        .try_into()
                        .unwrap(),
                    total: 0,
                    last_fetched: None,
                    last_error: None,
                    archived: false,
                })
                .collect())
        }
        fn items(&self, q: &ItemQuery) -> Result<ItemPage> {
            let items = self
                .items
                .borrow()
                .iter()
                .filter(|i| q.feed.as_ref().is_none_or(|f| *f == i.feed_id))
                .filter(|i| q.unread.is_none_or(|u| u != i.read))
                .filter(|i| q.starred.is_none_or(|s| s == i.starred))
                .filter(|i| q.q.as_ref().is_none_or(|s| i.title.contains(s.as_str())))
                .cloned()
                .collect();
            Ok(ItemPage {
                items,
                next_cursor: None,
                next_offset: None,
            })
        }
        fn item(&self, id: i64) -> Result<Item> {
            let mut it = self
                .items
                .borrow()
                .iter()
                .find(|i| i.id == id)
                .cloned()
                .unwrap();
            it.content = Some("<p>body</p>".into());
            Ok(it)
        }
        fn set_read(&self, id: i64, value: bool) -> Result<()> {
            self.calls.borrow_mut().push(format!("read {id} {value}"));
            self.items
                .borrow_mut()
                .iter_mut()
                .filter(|i| i.id == id)
                .for_each(|i| i.read = value);
            Ok(())
        }
        fn set_starred(&self, id: i64, value: bool) -> Result<()> {
            self.calls.borrow_mut().push(format!("star {id} {value}"));
            self.items
                .borrow_mut()
                .iter_mut()
                .filter(|i| i.id == id)
                .for_each(|i| i.starred = value);
            Ok(())
        }
        fn mark_feed_read(&self, feed: &str) -> Result<u32> {
            self.calls.borrow_mut().push(format!("read-all {feed}"));
            let mut n = 0;
            for i in self
                .items
                .borrow_mut()
                .iter_mut()
                .filter(|i| i.feed_id == feed && !i.read)
            {
                i.read = true;
                n += 1;
            }
            Ok(n)
        }
        fn refresh(&self) -> Result<()> {
            self.calls.borrow_mut().push("refresh".into());
            Ok(())
        }
    }

    fn keys(app: &mut App, fake: &Fake, keys: &str) {
        for c in keys.chars() {
            let key = match c {
                '\n' => KeyCode::Enter,
                '<' => KeyCode::Esc,
                '\x08' => KeyCode::Backspace,
                c => KeyCode::Char(c),
            };
            app.handle(key, fake);
        }
    }

    #[test]
    fn reading_marks_read_and_updates_counts() {
        let fake = Fake::new();
        let mut app = App::new();
        app.reload(&fake);
        assert_eq!((app.items.len(), app.total_unread()), (3, 3));

        keys(&mut app, &fake, "\nj\n");
        assert_eq!(app.focus, Pane::Reader);
        let reader = app.reader.as_ref().unwrap();
        assert_eq!(reader.item.id, 2);
        assert_eq!(reader.item.content.as_deref(), Some("<p>body</p>"));
        assert!(reader.item.read);
        assert_eq!(app.total_unread(), 2);

        keys(&mut app, &fake, "ms<");
        assert_eq!(app.focus, Pane::Items);
        assert!(app.reader.is_none());
        assert!(!app.items[1].read && app.items[1].starred);
        assert_eq!(
            *fake.calls.borrow(),
            ["read 2 true", "read 2 false", "star 2 true"]
        );
    }

    #[test]
    fn feed_selection_filters_and_starred_ignores_unread_filter() {
        let fake = Fake::new();
        let mut app = App::new();
        app.reload(&fake);
        keys(&mut app, &fake, "jjj");
        assert_eq!(app.feed_row, FIXED_ROWS + 1);
        assert!(app.items.iter().all(|i| i.feed_id == "b"));

        fake.set_starred(1, true).unwrap();
        fake.set_read(1, true).unwrap();
        keys(&mut app, &fake, "kk");
        assert_eq!(app.feed_row, STARRED_ROW);
        assert_eq!(app.items.len(), 1, "read but starred item still shows");

        keys(&mut app, &fake, "k");
        assert_eq!(app.items.len(), 2, "unread filter hides item 1");
        keys(&mut app, &fake, "u");
        assert_eq!(app.items.len(), 3);
    }

    #[test]
    fn mark_all_read_and_open() {
        let fake = Fake::new();
        let mut app = App::new();
        app.reload(&fake);
        keys(&mut app, &fake, "jj");
        keys(&mut app, &fake, "A");
        assert_eq!(app.status, "marked 2 items read");
        assert_eq!(app.total_unread(), 1);

        keys(&mut app, &fake, "jl");
        let action = app.handle(KeyCode::Char('o'), &fake);
        assert!(matches!(action, Action::Open(url) if url == "https://example.com/3"));
        assert_eq!(app.total_unread(), 0);
        assert!(matches!(
            app.handle(KeyCode::Char('q'), &fake),
            Action::Quit
        ));
    }

    #[test]
    fn search_prompt() {
        let fake = Fake::new();
        let mut app = App::new();
        app.reload(&fake);
        fake.set_read(1, true).unwrap();

        // Typing does not trigger other keys; Enter runs the search.
        keys(&mut app, &fake, "/item 1x\x08");
        assert_eq!(app.editing.as_deref(), Some("item 1"));
        assert_eq!(app.items.len(), 3, "nothing reloaded while typing");
        keys(&mut app, &fake, "\n");
        assert_eq!(app.search.as_deref(), Some("item 1"));
        assert_eq!(app.focus, Pane::Items);
        assert_eq!(
            app.items.iter().map(|i| i.id).collect::<Vec<_>>(),
            [1],
            "searches include read items"
        );

        // Esc clears the search; an empty search is no search.
        keys(&mut app, &fake, "<");
        assert_eq!(app.search, None);
        assert_eq!(app.items.len(), 2);
        keys(&mut app, &fake, "/zzz\n");
        assert_eq!(app.status, "no matches");
        keys(&mut app, &fake, "/\x08\x08\x08\n");
        assert_eq!(app.search, None);
        keys(&mut app, &fake, "/abc<");
        assert_eq!(
            (app.editing.clone(), app.search.clone()),
            (None, None),
            "Esc cancels typing"
        );
    }
}
