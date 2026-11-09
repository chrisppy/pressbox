// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Client state and key handling, kept apart from drawing so it can be
//! tested without a terminal.

use anyhow::Result;
use pressbox_api::{Feed, Item, ItemPage, ItemQuery, MarkRead};
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
    fn mark_read(&self, request: &MarkRead) -> Result<u32>;
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
    fn mark_read(&self, request: &MarkRead) -> Result<u32> {
        Client::mark_read(self, request)
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

/// Which items the list shows, by read state. Starred lists and searches
/// show both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Unread,
    All,
    Read,
}

impl Show {
    /// The next in the cycle `u` steps through.
    fn next(self) -> Self {
        match self {
            Self::Unread => Self::All,
            Self::All => Self::Read,
            Self::Read => Self::Unread,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::All => "all",
            Self::Read => "read",
        }
    }

    /// The `unread` query parameter: `false` asks for read items only.
    fn unread(self) -> Option<bool> {
        match self {
            Self::Unread => Some(true),
            Self::All => None,
            Self::Read => Some(false),
        }
    }
}

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
    pub show: Show,
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
            show: Show::Unread,
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

    /// Whether `show` applies: starred lists and searches ignore it.
    pub fn filters_by_read(&self) -> bool {
        self.feed_row != STARRED_ROW && self.search.is_none()
    }

    fn query(&self, from: Option<&NextPage>) -> ItemQuery {
        let starred = self.feed_row == STARRED_ROW;
        ItemQuery {
            feed: self.selected_feed().map(|f| f.id.clone()),
            starred: starred.then_some(true),
            unread: self.filters_by_read().then(|| self.show.unread()).flatten(),
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
        let text = self.editing.get_or_insert_default();
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
                KeyCode::Char('M') => self.mark_below_read(backend)?,
                KeyCode::Char('u') => {
                    self.show = self.show.next();
                    self.item_row = 0;
                    self.load_items(backend)?;
                    self.status = format!("showing {} items", self.show.label());
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

    /// Sets the read flag of the current item, locally and on the server,
    /// unless it is already set that way.
    fn mark(&mut self, backend: &impl Backend, read: bool) -> Result<()> {
        let Some(item) = self.current_item().filter(|i| i.read != read) else {
            return Ok(());
        };
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

    /// Marks the selected item and every item below it in the list read,
    /// including pages not loaded yet, in one request.
    fn mark_below_read(&mut self, backend: &impl Backend) -> Result<()> {
        if self.focus == Pane::Feeds {
            return Ok(());
        }
        let Some(from) = self.items.get(self.item_row).map(|i| i.id) else {
            return Ok(());
        };
        let marked = backend.mark_read(&MarkRead {
            query: ItemQuery {
                limit: None,
                ..self.query(None)
            },
            from: Some(from),
        })?;
        // Left in the list, as `m` leaves them, until the next reload.
        for item in &mut self.items[self.item_row..] {
            item.read = true;
        }
        if let Some(r) = self.reader.as_mut() {
            r.item.read |= self.items[self.item_row..]
                .iter()
                .any(|i| i.id == r.item.id);
        }
        self.feeds = backend.feeds()?;
        let s = if marked == 1 { "" } else { "s" };
        self.status = format!("marked {marked} item{s} read");
        Ok(())
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
        /// Items per page, and whether pages continue by offset (an older
        /// server) rather than cursor. All items on one page if `None`.
        paging: std::cell::Cell<Option<(usize, bool)>>,
        /// Item lists fail while set.
        offline: std::cell::Cell<bool>,
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
                paging: std::cell::Cell::new(None),
                offline: std::cell::Cell::new(false),
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
            if self.offline.get() {
                anyhow::bail!("offline");
            }
            let mut items: Vec<Item> = self
                .items
                .borrow()
                .iter()
                .filter(|i| q.feed.as_ref().is_none_or(|f| *f == i.feed_id))
                .filter(|i| q.unread.is_none_or(|u| u != i.read))
                .filter(|i| q.starred.is_none_or(|s| s == i.starred))
                .filter(|i| q.q.as_ref().is_none_or(|s| i.title.contains(s.as_str())))
                .cloned()
                .collect();
            let Some((size, offsets)) = self.paging.get() else {
                return Ok(ItemPage {
                    items,
                    next_cursor: None,
                    next_offset: None,
                });
            };
            let start = match (&q.cursor, q.offset) {
                (Some(after), _) => {
                    1 + items
                        .iter()
                        .position(|i| i.id.to_string() == *after)
                        .unwrap()
                }
                (None, offset) => offset.unwrap_or(0) as usize,
            };
            let mut page = items.split_off(start);
            let more = page.len() > size;
            page.truncate(size);
            let end = (start + page.len()) as u32;
            Ok(ItemPage {
                next_cursor: (more && !offsets).then(|| page.last().unwrap().id.to_string()),
                next_offset: (more && offsets).then_some(end),
                items: page,
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
        fn mark_read(&self, request: &MarkRead) -> Result<u32> {
            self.calls
                .borrow_mut()
                .push(format!("mark-read from {:?}", request.from));
            let listed: Vec<i64> = self
                .items(&request.query)?
                .items
                .iter()
                .map(|i| i.id)
                .collect();
            let start = request
                .from
                .map_or(Some(0), |from| listed.iter().position(|&id| id == from));
            let Some(start) = start else { return Ok(0) };
            let mut n = 0;
            for id in &listed[start..] {
                if let Some(i) = self
                    .items
                    .borrow_mut()
                    .iter_mut()
                    .find(|i| i.id == *id && !i.read)
                {
                    i.read = true;
                    n += 1;
                }
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
    fn marks_below_read_and_shows_read_items() {
        let fake = Fake::new();
        let mut app = App::new();
        app.reload(&fake);
        keys(&mut app, &fake, "M");
        assert!(
            fake.calls.borrow().is_empty(),
            "nothing selected in the feed list"
        );

        // From item 2 down (2 and 3), not item 1 above it.
        keys(&mut app, &fake, "\njM");
        assert_eq!(*fake.calls.borrow(), ["mark-read from Some(2)"]);
        assert_eq!(app.status, "marked 2 items read");
        let read: Vec<bool> = app.items.iter().map(|i| i.read).collect();
        assert_eq!(read, [false, true, true], "kept in the list until reload");
        assert_eq!(app.total_unread(), 1);

        // u cycles unread -> all -> read -> unread.
        keys(&mut app, &fake, "u");
        assert_eq!((app.show, app.items.len()), (Show::All, 3));
        assert_eq!(app.status, "showing all items");
        keys(&mut app, &fake, "u");
        assert_eq!(app.show, Show::Read);
        assert_eq!(app.items.iter().map(|i| i.id).collect::<Vec<_>>(), [2, 3]);
        keys(&mut app, &fake, "u");
        assert_eq!((app.show, app.items.len()), (Show::Unread, 1));

        // Starred lists ignore it.
        keys(&mut app, &fake, "uu");
        fake.set_starred(1, true).unwrap();
        keys(&mut app, &fake, "hj");
        assert_eq!(app.feed_row, STARRED_ROW);
        assert_eq!(app.items.len(), 1, "unread starred item shows in read view");
    }

    #[test]
    fn works_against_a_real_client() {
        let feeds = r#"[{"id":"a","title":"A","url":"https://a.example/","site_url":null,
            "tags":[],"unread":1,"total":1,"last_fetched":null,"last_error":null,"archived":false}]"#;
        let page = r#"{"items":[{"id":7,"feed_id":"a","title":"Seven","url":null,"author":null,
            "summary":null,"published":0,"read":false,"starred":false}],"next_cursor":null}"#;
        let item = r#"{"id":7,"feed_id":"a","title":"Seven","url":null,"author":null,
            "summary":null,"content":"<p>x</p>","published":0,"read":false,"starred":false}"#;
        let (base, requests) = crate::test_server::server(vec![
            (200, feeds),
            (200, page),
            (202, ""),
            (200, item),
            (204, ""),
            (204, ""),
            (200, r#"{"updated":2}"#),
            (200, r#"{"updated":3}"#),
            (401, "{}"),
        ]);
        let client = Client::new(&base, Some("t".into())).unwrap();
        let mut app = App::new();
        app.reload(&client);
        assert_eq!(
            (app.total_unread(), app.items[0].title.as_str()),
            (1, "Seven")
        );
        let _ = app.handle(KeyCode::Char('R'), &client);
        assert!(
            app.status.starts_with("server is refreshing"),
            "{}",
            app.status
        );
        assert!(Backend::item(&client, 7).is_ok());
        Backend::set_read(&client, 7, true).unwrap();
        Backend::set_starred(&client, 7, true).unwrap();
        assert_eq!(Backend::mark_feed_read(&client, "a").unwrap(), 2);
        assert_eq!(
            Backend::mark_read(&client, &MarkRead::default()).unwrap(),
            3
        );
        app.reload(&client);
        assert_eq!(app.status, "error: the server rejected the token (401)");
        let paths: Vec<String> = requests
            .iter()
            .take(9)
            .map(|h| h.split(' ').nth(1).unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                "/api/v1/feeds",
                "/api/v1/items?unread=true&limit=100",
                "/api/v1/refresh",
                "/api/v1/items/7",
                "/api/v1/items/7/read",
                "/api/v1/items/7/starred",
                "/api/v1/feeds/a/read-all",
                "/api/v1/items/read-all",
                "/api/v1/feeds"
            ]
        );
    }

    #[test]
    fn pages_in_as_you_scroll_and_keeps_its_place_on_errors() {
        for offsets in [false, true] {
            let fake = Fake::new();
            fake.paging.set(Some((1, offsets)));
            let mut app = App::new();
            app.reload(&fake);
            let more = if offsets {
                NextPage::Offset(1)
            } else {
                NextPage::Cursor("1".into())
            };
            assert_eq!(
                (app.items.len(), app.next_page.clone()),
                (1, Some(more.clone()))
            );

            // Near the end of the list, the next page loads.
            keys(&mut app, &fake, "\n");
            fake.offline.set(true);
            keys(&mut app, &fake, "j");
            assert_eq!(app.status, "error: offline");
            assert_eq!(app.next_page, Some(more), "tried again next time");
            fake.offline.set(false);
            keys(&mut app, &fake, "jj");
            assert_eq!(
                app.items.iter().map(|i| i.id).collect::<Vec<_>>(),
                [1, 2, 3]
            );
            assert_eq!(app.next_page, None);
        }
    }

    #[test]
    fn moves_scrolls_and_ignores_what_it_cant_do() {
        let fake = Fake::new();
        let mut app = App::new();
        // Nothing loaded: nothing to open, mark or star.
        keys(&mut app, &fake, "\nlmsoM");
        assert!(fake.calls.borrow().is_empty());
        assert!(app.reader.is_none());

        app.reload(&fake);
        keys(&mut app, &fake, "z");
        assert_eq!(app.status, "", "unknown keys do nothing");
        keys(&mut app, &fake, "r");
        assert_eq!(app.status, "reloaded");

        // Page keys move ten at a time, within the list.
        let page = |app: &mut App, key| app.handle(key, &fake);
        assert_eq!(app.focus, Pane::Items);
        let _ = page(&mut app, KeyCode::PageDown);
        assert_eq!(app.item_row, 2);
        let _ = page(&mut app, KeyCode::PageUp);
        assert_eq!(app.item_row, 0);
        keys(&mut app, &fake, " ");
        assert_eq!(app.item_row, 2);

        // Tab opens the reader; there, keys scroll and Enter/Tab stay put.
        let _ = page(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Reader);
        keys(&mut app, &fake, "jj\nl");
        assert_eq!(app.reader.as_ref().unwrap().scroll, 2);
        keys(&mut app, &fake, "k");
        assert_eq!(app.reader.as_ref().unwrap().scroll, 1);
        // `o` on an item already read doesn't mark it again.
        let calls = fake.calls.borrow().len();
        let _ = app.handle(KeyCode::Char('o'), &fake);
        assert_eq!(fake.calls.borrow().len(), calls);

        // M in the reader covers the open item too.
        keys(&mut app, &fake, "<k");
        fake.set_read(3, false).unwrap();
        app.items[2].read = false;
        let _ = page(&mut app, KeyCode::Tab);
        keys(&mut app, &fake, "m");
        assert!(!app.reader.as_ref().unwrap().item.read);
        keys(&mut app, &fake, "M");
        assert!(app.reader.as_ref().unwrap().item.read);

        // A on "All items" marks every feed with unread items.
        keys(&mut app, &fake, "<<hkkk");
        assert_eq!(app.feed_row, ALL_ROW);
        fake.set_read(1, false).unwrap();
        app.reload(&fake);
        keys(&mut app, &fake, "A");
        assert_eq!(fake.calls.borrow().last().unwrap(), "read-all a");

        // A in the starred list does nothing; Tab in the search prompt
        // is ignored.
        keys(&mut app, &fake, "<<hkkj");
        assert_eq!(app.feed_row, STARRED_ROW);
        let calls = fake.calls.borrow().len();
        keys(&mut app, &fake, "A");
        assert_eq!(fake.calls.borrow().len(), calls);
        keys(&mut app, &fake, "/x");
        let _ = page(&mut app, KeyCode::Tab);
        assert_eq!(app.editing.as_deref(), Some("x"));
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
