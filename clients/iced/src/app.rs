// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! The application: state, messages and updates. The layout is in `ui.rs`.

use iced::{Subscription, Task, Theme, keyboard};
use pressbox_api::{Feed, Item, ItemPage, ItemQuery, Me, PairApproved, PairStart, Version};

use crate::{
    api::{self, Client},
    config::{self, ClientConfig, Session},
    theme,
};

/// Items fetched per page.
const PAGE: u32 = 50;

pub struct App {
    pub config: ClientConfig,
    pub screen: Screen,
    pub dark: bool,
}

pub enum Screen {
    Setup(Setup),
    Reader(Box<Reader>),
}

#[derive(Default)]
pub struct Setup {
    pub server: String,
    pub device: String,
    /// The code to approve, once pairing has started.
    pub code: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}

/// What the item list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Unread,
    All,
    Starred,
    Feed(String),
    Tag(String),
}

pub struct Reader {
    pub client: Client,
    pub me: Option<Me>,
    pub version: Option<Version>,
    pub feeds: Vec<Feed>,
    pub source: Source,
    pub search: String,
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
    pub loading: bool,
    /// Bumped whenever the list is reloaded, so late pages of an old list
    /// are dropped.
    generation: u64,
    pub selected: Option<i64>,
    /// The selected item with its full content, once fetched.
    pub article: Option<Item>,
    pub article_text: String,
    pub approve_code: String,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Read,
    Starred,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Setup
    ServerChanged(String),
    DeviceChanged(String),
    Pair,
    PairStarted(api::Result<PairStart>),
    PairDone(api::Result<PairApproved>),

    // Reader
    Identity(api::Result<(Me, Version)>),
    FeedsLoaded(api::Result<Vec<Feed>>),
    Select(Source),
    SearchChanged(String),
    SearchSubmit,
    ItemsLoaded(u64, bool, api::Result<ItemPage>),
    LoadMore,
    OpenItem(i64),
    ItemLoaded(api::Result<Item>),
    Toggle(Flag),
    Flagged(i64, Flag, bool, api::Result<()>),
    OpenInBrowser,
    MarkAllRead,
    MarkedAllRead(api::Result<u32>),
    Refresh,
    Refreshed(api::Result<()>),
    ApproveCodeChanged(String),
    Approve,
    Approved(api::Result<pressbox_api::Approved>),
    Next,
    Previous,
    Dismiss,
    SignOut,

    ThemeMode(iced::theme::Mode),
}

impl App {
    pub fn boot() -> (Self, Task<Message>) {
        let (config, session) = config::load_session();
        let mut app = Self {
            screen: Screen::Setup(Setup::default()),
            config,
            dark: true,
        };
        let theme = iced::system::theme().map(Message::ThemeMode);
        let task = match session.and_then(|s| Client::new(&s.server, Some(s.token)).ok()) {
            Some(client) => app.open_reader(client),
            None => {
                app.screen = Screen::Setup(app.new_setup());
                Task::none()
            }
        };
        (app, Task::batch([theme, task]))
    }

    fn new_setup(&self) -> Setup {
        Setup {
            server: self.config.server.clone().unwrap_or_default(),
            device: default_device_name(),
            ..Setup::default()
        }
    }

    fn open_reader(&mut self, client: Client) -> Task<Message> {
        let mut reader = Reader::new(client.clone());
        let identity = Task::perform(
            {
                let client = client.clone();
                async move {
                    let me = client.clone().me().await?;
                    let version = client.version().await?;
                    Ok((me, version))
                }
            },
            Message::Identity,
        );
        let items = reader.reload();
        self.screen = Screen::Reader(Box::new(reader));
        Task::batch([identity, load_feeds(&client), items])
    }

    pub fn title(&self) -> String {
        match &self.screen {
            Screen::Reader(r) => {
                let unread: u32 = r.feeds.iter().map(|f| f.unread).sum();
                if unread > 0 {
                    format!("pressbox ({unread})")
                } else {
                    "pressbox".into()
                }
            }
            Screen::Setup(_) => "pressbox".into(),
        }
    }

    pub fn theme(&self) -> Theme {
        if self.dark {
            theme::DARK.theme(true)
        } else {
            theme::LIGHT.theme(false)
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mode = iced::system::theme_changes().map(Message::ThemeMode);
        let keys = match self.screen {
            Screen::Reader(_) => keyboard::listen().filter_map(on_key),
            Screen::Setup(_) => Subscription::none(),
        };
        Subscription::batch([mode, keys])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::ThemeMode(mode) = message {
            self.dark = !matches!(mode, iced::theme::Mode::Light);
            return Task::none();
        }
        match &mut self.screen {
            Screen::Setup(setup) => match message {
                Message::ServerChanged(s) => {
                    setup.server = s;
                    Task::none()
                }
                Message::DeviceChanged(s) => {
                    setup.device = s;
                    Task::none()
                }
                Message::Pair => {
                    if setup.server.trim().is_empty() || setup.device.trim().is_empty() {
                        setup.error = Some("Enter the server address and a device name.".into());
                        return Task::none();
                    }
                    let client = match Client::new(&setup.server, None) {
                        Ok(c) => c,
                        Err(e) => {
                            setup.error = Some(e);
                            return Task::none();
                        }
                    };
                    setup.busy = true;
                    setup.error = None;
                    setup.code = None;
                    Task::perform(
                        client.pair_start(setup.device.trim().to_owned()),
                        Message::PairStarted,
                    )
                }
                Message::PairStarted(Ok(start)) => {
                    setup.code = Some(start.code);
                    match Client::new(&setup.server, None) {
                        Ok(client) => {
                            Task::perform(client.pair_wait(start.poll_id), Message::PairDone)
                        }
                        Err(e) => {
                            setup.busy = false;
                            setup.error = Some(e);
                            Task::none()
                        }
                    }
                }
                Message::PairStarted(Err(e)) | Message::PairDone(Err(e)) => {
                    setup.busy = false;
                    setup.code = None;
                    setup.error = Some(e);
                    Task::none()
                }
                Message::PairDone(Ok(approved)) => {
                    let session = Session {
                        server: api::normalize_server(&setup.server),
                        token: approved.token,
                    };
                    if let Err(e) = config::save_session(&self.config, &session) {
                        setup.busy = false;
                        setup.error = Some(format!("Paired, but could not save the token: {e:#}"));
                        return Task::none();
                    }
                    self.config.server = Some(session.server.clone());
                    match Client::new(&session.server, Some(session.token)) {
                        Ok(client) => self.open_reader(client),
                        Err(e) => {
                            setup.busy = false;
                            setup.error = Some(e);
                            Task::none()
                        }
                    }
                }
                _ => Task::none(),
            },
            Screen::Reader(reader) => {
                if let Message::SignOut = message {
                    if let Some(path) = self.config.token_path() {
                        let _ = std::fs::remove_file(path);
                    }
                    self.screen = Screen::Setup(self.new_setup());
                    return Task::none();
                }
                reader.update(message)
            }
        }
    }
}

impl Reader {
    /// An empty reader showing unread items; nothing is loaded yet.
    fn new(client: Client) -> Self {
        Self {
            client,
            me: None,
            version: None,
            feeds: Vec::new(),
            source: Source::Unread,
            search: String::new(),
            items: Vec::new(),
            next_cursor: None,
            loading: false,
            generation: 0,
            selected: None,
            article: None,
            article_text: String::new(),
            approve_code: String::new(),
            status: None,
            error: None,
        }
    }

    fn query(&self) -> ItemQuery {
        let mut query = ItemQuery {
            limit: Some(PAGE),
            ..ItemQuery::default()
        };
        match &self.source {
            Source::Unread => query.unread = Some(true),
            Source::All => {}
            Source::Starred => query.starred = Some(true),
            Source::Feed(f) => query.feed = Some(f.clone()),
            Source::Tag(t) => query.tag = Some(t.clone()),
        }
        let q = self.search.trim();
        if !q.is_empty() {
            query.q = Some(q.to_owned());
        }
        query
    }

    /// Starts the list over from the first page.
    fn reload(&mut self) -> Task<Message> {
        self.generation += 1;
        self.loading = true;
        self.next_cursor = None;
        let generation = self.generation;
        Task::perform(self.client.clone().items(self.query()), move |r| {
            Message::ItemsLoaded(generation, false, r)
        })
    }

    pub fn tags(&self) -> Vec<(String, u32)> {
        let mut tags: Vec<(String, u32)> = Vec::new();
        for feed in &self.feeds {
            for tag in &feed.tags {
                match tags.iter_mut().find(|(t, _)| t == tag) {
                    Some((_, n)) => *n += feed.unread,
                    None => tags.push((tag.clone(), feed.unread)),
                }
            }
        }
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        tags
    }

    pub fn feed_title(&self, id: &str) -> String {
        self.feeds
            .iter()
            .find(|f| f.id == id)
            .map_or_else(|| id.to_owned(), |f| f.title.clone())
    }

    pub fn source_title(&self) -> String {
        match &self.source {
            Source::Unread => "Unread".into(),
            Source::All => "All items".into(),
            Source::Starred => "Starred".into(),
            Source::Feed(f) => self.feed_title(f),
            Source::Tag(t) => format!("#{t}"),
        }
    }

    fn item_mut(&mut self, id: i64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Sets a flag locally, adjusting the feed's unread count.
    fn apply(&mut self, id: i64, flag: Flag, value: bool) {
        let mut feed_id = None;
        let mut changed = false;
        for item in self
            .items
            .iter_mut()
            .chain(self.article.iter_mut())
            .filter(|i| i.id == id)
        {
            let slot = match flag {
                Flag::Read => &mut item.read,
                Flag::Starred => &mut item.starred,
            };
            if *slot != value {
                *slot = value;
                changed = true;
                feed_id = Some(item.feed_id.clone());
            }
        }
        if changed
            && flag == Flag::Read
            && let Some(feed) = self
                .feeds
                .iter_mut()
                .find(|f| Some(&f.id) == feed_id.as_ref())
        {
            feed.unread = if value {
                feed.unread.saturating_sub(1)
            } else {
                feed.unread + 1
            };
        }
    }

    fn set_flag(&mut self, id: i64, flag: Flag, value: bool) -> Task<Message> {
        self.apply(id, flag, value);
        let client = self.client.clone();
        let fut = async move {
            match flag {
                Flag::Read => client.set_read(id, value).await,
                Flag::Starred => client.set_starred(id, value).await,
            }
        };
        Task::perform(fut, move |r| Message::Flagged(id, flag, value, r))
    }

    fn open(&mut self, id: i64) -> Task<Message> {
        self.selected = Some(id);
        self.article = self.items.iter().find(|i| i.id == id).cloned();
        self.article_text = self
            .article
            .as_ref()
            .map(|i| render_html(i.summary.as_deref().unwrap_or_default()))
            .unwrap_or_default();
        let fetch = Task::perform(self.client.clone().item(id), Message::ItemLoaded);
        let unread = self.item_mut(id).is_some_and(|i| !i.read);
        if unread {
            Task::batch([fetch, self.set_flag(id, Flag::Read, true)])
        } else {
            fetch
        }
    }

    fn step(&mut self, by: isize) -> Task<Message> {
        let index = self
            .selected
            .and_then(|id| self.items.iter().position(|i| i.id == id));
        let next = match index {
            Some(i) => i.checked_add_signed(by),
            None => Some(0),
        };
        match next.and_then(|i| self.items.get(i)).map(|i| i.id) {
            Some(id) => self.open(id),
            None if by > 0 && self.next_cursor.is_some() && !self.loading => {
                self.update(Message::LoadMore)
            }
            None => Task::none(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Identity(Ok((me, version))) => {
                self.me = Some(me);
                self.version = Some(version);
                Task::none()
            }
            Message::FeedsLoaded(Ok(feeds)) => {
                self.feeds = feeds;
                Task::none()
            }
            Message::Identity(Err(e))
            | Message::FeedsLoaded(Err(e))
            | Message::ItemLoaded(Err(e))
            | Message::MarkedAllRead(Err(e))
            | Message::Refreshed(Err(e))
            | Message::Approved(Err(e)) => {
                self.error = Some(e);
                Task::none()
            }
            Message::Select(source) => {
                self.source = source;
                self.selected = None;
                self.article = None;
                self.reload()
            }
            Message::SearchChanged(s) => {
                self.search = s;
                Task::none()
            }
            Message::SearchSubmit => self.reload(),
            Message::ItemsLoaded(generation, append, result) => {
                if generation != self.generation {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(page) => {
                        if !append {
                            self.items.clear();
                        }
                        self.items.extend(page.items);
                        self.next_cursor = page.next_cursor;
                    }
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }
            Message::LoadMore => {
                let Some(cursor) = self.next_cursor.clone() else {
                    return Task::none();
                };
                self.loading = true;
                let generation = self.generation;
                let query = ItemQuery {
                    cursor: Some(cursor),
                    ..self.query()
                };
                Task::perform(self.client.clone().items(query), move |r| {
                    Message::ItemsLoaded(generation, true, r)
                })
            }
            Message::OpenItem(id) => self.open(id),
            Message::ItemLoaded(Ok(mut item)) => {
                if self.selected == Some(item.id) {
                    // The fetch can race a flag change made since; the
                    // local state is the newer one.
                    if let Some(local) = self.items.iter().find(|i| i.id == item.id) {
                        item.read = local.read;
                        item.starred = local.starred;
                    }
                    let html = item
                        .content
                        .as_deref()
                        .or(item.summary.as_deref())
                        .unwrap_or_default();
                    self.article_text = render_html(html);
                    self.article = Some(item);
                }
                Task::none()
            }
            Message::Toggle(flag) => match &self.article {
                Some(item) => {
                    let value = match flag {
                        Flag::Read => !item.read,
                        Flag::Starred => !item.starred,
                    };
                    self.set_flag(item.id, flag, value)
                }
                None => Task::none(),
            },
            Message::Flagged(id, flag, value, Err(e)) => {
                self.apply(id, flag, !value);
                self.error = Some(e);
                Task::none()
            }
            Message::Flagged(_, _, _, Ok(())) => Task::none(),
            Message::OpenInBrowser => {
                if let Some(url) = self.article.as_ref().and_then(|i| i.url.clone())
                    && let Err(e) = open::that_detached(&url)
                {
                    self.error = Some(format!("could not open {url}: {e}"));
                }
                Task::none()
            }
            Message::MarkAllRead => {
                let feeds: Vec<String> = match &self.source {
                    Source::Feed(f) => vec![f.clone()],
                    Source::Tag(t) => self
                        .feeds
                        .iter()
                        .filter(|f| f.tags.contains(t) && f.unread > 0)
                        .map(|f| f.id.clone())
                        .collect(),
                    Source::Unread | Source::All => self
                        .feeds
                        .iter()
                        .filter(|f| f.unread > 0)
                        .map(|f| f.id.clone())
                        .collect(),
                    Source::Starred => Vec::new(),
                };
                if feeds.is_empty() {
                    return Task::none();
                }
                let client = self.client.clone();
                Task::perform(
                    async move {
                        let mut total = 0;
                        for feed in feeds {
                            total += client.clone().mark_feed_read(feed).await?;
                        }
                        Ok(total)
                    },
                    Message::MarkedAllRead,
                )
            }
            Message::MarkedAllRead(Ok(n)) => {
                self.status = Some(format!(
                    "Marked {n} item{} as read",
                    if n == 1 { "" } else { "s" }
                ));
                Task::batch([load_feeds(&self.client), self.reload()])
            }
            Message::Refresh => {
                self.status = Some("Refreshing feeds…".into());
                Task::perform(self.client.clone().refresh(), Message::Refreshed)
            }
            Message::Refreshed(Ok(())) => {
                self.status = None;
                Task::batch([load_feeds(&self.client), self.reload()])
            }
            Message::ApproveCodeChanged(s) => {
                self.approve_code = s.to_uppercase();
                Task::none()
            }
            Message::Approve => {
                let code = self.approve_code.trim().to_owned();
                if code.is_empty() {
                    return Task::none();
                }
                Task::perform(self.client.clone().approve(code, None), Message::Approved)
            }
            Message::Approved(Ok(approved)) => {
                self.approve_code.clear();
                self.status = Some(format!("Approved {} for {}", approved.name, approved.user));
                Task::none()
            }
            Message::Next => self.step(1),
            Message::Previous => self.step(-1),
            Message::Dismiss => {
                self.error = None;
                self.status = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }
}

fn load_feeds(client: &Client) -> Task<Message> {
    Task::perform(client.clone().feeds(), Message::FeedsLoaded)
}

fn on_key(event: keyboard::Event) -> Option<Message> {
    use keyboard::{Key, key::Named};
    let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        Key::Character("j") | Key::Named(Named::ArrowDown) => Some(Message::Next),
        Key::Character("k") | Key::Named(Named::ArrowUp) => Some(Message::Previous),
        Key::Character("s") => Some(Message::Toggle(Flag::Starred)),
        Key::Character("m") => Some(Message::Toggle(Flag::Read)),
        Key::Character("o") => Some(Message::OpenInBrowser),
        Key::Named(Named::Escape) => Some(Message::Dismiss),
        _ => None,
    }
}

/// Article HTML as plain text, wrapped wide enough that iced does the
/// real wrapping.
pub fn render_html(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    html2text::from_read(html.as_bytes(), 200).unwrap_or_else(|_| html.to_owned())
}

fn default_device_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "desktop".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_html_as_text() {
        let text = render_html("<p>Hello <b>world</b></p><ul><li>one</li></ul>");
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(text.contains("one"));
        assert_eq!(render_html("  "), "");
    }

    fn reader() -> Reader {
        Reader::new(Client::new("http://127.0.0.1:9", Some("t".into())).unwrap())
    }

    fn feed(id: &str, unread: u32, tags: &[&str]) -> Feed {
        Feed {
            id: id.into(),
            title: format!("Feed {id}"),
            url: format!("https://{id}.example/"),
            site_url: None,
            tags: tags.iter().map(|&t| t.into()).collect(),
            unread,
            total: unread,
            last_fetched: None,
            last_error: None,
            archived: false,
        }
    }

    fn item(id: i64, feed: &str) -> Item {
        Item {
            id,
            feed_id: feed.into(),
            title: format!("Item {id}"),
            url: Some(format!("https://{feed}.example/{id}")),
            author: None,
            summary: Some(format!("<p>summary {id}</p>")),
            content: None,
            published: id,
            read: false,
            starred: false,
        }
    }

    fn page(ids: &[i64], next_cursor: Option<&str>) -> ItemPage {
        ItemPage {
            items: ids.iter().map(|&id| item(id, "a")).collect(),
            next_cursor: next_cursor.map(str::to_owned),
            next_offset: None,
        }
    }

    fn ids(r: &Reader) -> Vec<i64> {
        r.items.iter().map(|i| i.id).collect()
    }

    #[test]
    fn queries_follow_the_source_and_search() {
        let mut r = reader();
        assert_eq!(r.query().unread, Some(true));
        r.source = Source::Starred;
        r.search = "  rust  ".into();
        let q = r.query();
        assert_eq!(
            (q.starred, q.unread, q.q.as_deref()),
            (Some(true), None, Some("rust"))
        );
        r.source = Source::Tag("nix".into());
        r.search = "   ".into();
        let q = r.query();
        assert_eq!((q.tag.as_deref(), q.q), (Some("nix"), None));
        r.source = Source::Feed("a".into());
        assert_eq!(r.query().feed.as_deref(), Some("a"));
        assert_eq!(r.query().limit, Some(PAGE));
    }

    #[test]
    fn pages_append_and_stale_pages_are_dropped() {
        let mut r = reader();
        let _ = r.reload();
        let first = r.generation;
        let _ = r.update(Message::ItemsLoaded(
            first,
            false,
            Ok(page(&[3, 2], Some("c"))),
        ));
        assert_eq!(
            (ids(&r), r.next_cursor.as_deref(), r.loading),
            (vec![3, 2], Some("c"), false)
        );

        let _ = r.update(Message::LoadMore);
        assert!(r.loading);
        let _ = r.update(Message::ItemsLoaded(first, true, Ok(page(&[1], None))));
        assert_eq!((ids(&r), r.next_cursor.clone()), (vec![3, 2, 1], None));
        let _ = r.update(Message::LoadMore);
        assert!(!r.loading, "no cursor, nothing to load");

        // A page for a list that has since been reloaded is ignored.
        let _ = r.update(Message::Select(Source::All));
        let _ = r.update(Message::ItemsLoaded(first, false, Ok(page(&[9], None))));
        assert_eq!(ids(&r), [3, 2, 1]);
        assert!(r.loading);
        let _ = r.update(Message::ItemsLoaded(
            r.generation,
            false,
            Err("boom".into()),
        ));
        assert_eq!((r.error.as_deref(), r.loading), (Some("boom"), false));
        let _ = r.update(Message::Dismiss);
        assert_eq!(r.error, None);
    }

    #[test]
    fn opening_marks_read_and_keeps_counts() {
        let mut r = reader();
        r.feeds = vec![feed("a", 2, &[])];
        r.items = vec![item(2, "a"), item(1, "a")];
        let _ = r.update(Message::Next);
        assert_eq!(
            r.selected,
            Some(2),
            "Next with nothing open opens the first"
        );
        assert!(r.items[0].read && r.article.as_ref().unwrap().read);
        assert!(r.article_text.contains("summary 2"));
        assert_eq!(r.feeds[0].unread, 1);

        // A failed flag is put back.
        let _ = r.update(Message::Flagged(2, Flag::Read, true, Err("offline".into())));
        assert!(!r.items[0].read);
        assert_eq!(
            (r.feeds[0].unread, r.error.as_deref()),
            (2, Some("offline"))
        );

        // The full item replaces the summary, but local flags win a race.
        r.items[0].starred = true;
        let mut full = item(2, "a");
        full.content = Some("<p>full text</p>".into());
        let _ = r.update(Message::ItemLoaded(Ok(full)));
        assert!(r.article_text.contains("full text"));
        assert!(r.article.as_ref().unwrap().starred);
        let _ = r.update(Message::ItemLoaded(Ok(item(1, "a"))));
        assert_eq!(r.article.as_ref().unwrap().id, 2, "not the open item");

        let _ = r.update(Message::Toggle(Flag::Starred));
        assert!(!r.items[0].starred && !r.article.as_ref().unwrap().starred);
        let _ = r.update(Message::Next);
        assert_eq!(r.selected, Some(1));
        let _ = r.update(Message::Next);
        assert_eq!(r.selected, Some(1), "stays on the last item");
        let _ = r.update(Message::Previous);
        assert_eq!(r.selected, Some(2));
    }

    #[test]
    fn tags_sum_unread_counts() {
        let mut r = reader();
        r.feeds = vec![
            feed("a", 2, &["tech", "news"]),
            feed("b", 3, &["tech"]),
            feed("c", 0, &[]),
        ];
        assert_eq!(r.tags(), [("news".to_owned(), 2), ("tech".to_owned(), 5)]);
        assert_eq!(r.feed_title("b"), "Feed b");
        assert_eq!(r.feed_title("zz"), "zz");
        r.source = Source::Tag("tech".into());
        assert_eq!(r.source_title(), "#tech");
        r.source = Source::Feed("a".into());
        assert_eq!(r.source_title(), "Feed a");
    }

    #[test]
    fn approve_codes_are_uppercased() {
        let mut r = reader();
        let _ = r.update(Message::ApproveCodeChanged("k7w-4qx".into()));
        assert_eq!(r.approve_code, "K7W-4QX");
        let _ = r.update(Message::Approved(Ok(pressbox_api::Approved {
            name: "phone".into(),
            user: "chris".into(),
        })));
        assert!(r.approve_code.is_empty());
        assert_eq!(r.status.as_deref(), Some("Approved phone for chris"));
        let _ = r.update(Message::MarkedAllRead(Ok(1)));
        assert_eq!(r.status.as_deref(), Some("Marked 1 item as read"));
    }

    #[test]
    fn keys() {
        use keyboard::{Key, Location, Modifiers, key::Named};
        let press = |key: Key, modifiers| keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        };
        let char = |c: &str| Key::Character(c.into());
        assert!(matches!(
            on_key(press(char("j"), Modifiers::empty())),
            Some(Message::Next)
        ));
        assert!(matches!(
            on_key(press(Key::Named(Named::ArrowUp), Modifiers::empty())),
            Some(Message::Previous)
        ));
        assert!(matches!(
            on_key(press(char("s"), Modifiers::empty())),
            Some(Message::Toggle(Flag::Starred))
        ));
        assert!(
            on_key(press(char("j"), Modifiers::CTRL)).is_none(),
            "shortcuts pass"
        );
        assert!(on_key(press(char("x"), Modifiers::empty())).is_none());
    }
}
