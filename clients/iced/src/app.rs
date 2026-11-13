// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! The application: state, messages and updates. The layout is in `ui.rs`.

use iced::{Subscription, Task, Theme, keyboard};
use pressbox_api::{
    Chapter, Chapters, Feed, Item, ItemPage, ItemQuery, Me, PairApproved, PairStart, Transcript,
    Version,
};

use crate::{
    api::{self, Client},
    config::{self, ClientConfig, Session},
    controls::{self, MediaKey, Remote},
    player::{self, Loaded, Player, PlayerState},
    theme,
};

/// Items fetched per page.
const PAGE: u32 = 50;
/// While playing, the position is saved after it moves this many seconds.
const SAVE_EVERY: u32 = 15;
/// Stopping this close to the end (seconds) counts as finishing.
const NEAR_END: f64 = 30.0;
/// The speeds the speed button steps through.
pub const SPEEDS: [f64; 6] = [1.0, 1.25, 1.5, 1.75, 2.0, 0.75];

pub struct App {
    pub config: ClientConfig,
    /// Where `config` lives (`client.json`), if there is a config directory.
    pub config_path: Option<std::path::PathBuf>,
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
    /// The client pairing started with, once it has.
    pub client: Option<Client>,
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
    /// The Up Next queue, in playing order.
    UpNext,
    Feed(String),
    Tag(String),
}

/// A change to Up Next, about the open article's episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueChange {
    /// Adds it at the end, or takes it out if queued.
    Toggle,
    /// Puts it first.
    PlayNext,
    /// Moves it one place later (`1`) or earlier (`-1`).
    Move(isize),
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
    /// Opens a link in the browser; tests swap it out.
    open_url: fn(&str) -> std::io::Result<()>,
    /// The open article's chapters, if it has any.
    pub chapters: Vec<Chapter>,
    /// The open article's transcript, once loaded; `Some(None)`: none.
    pub transcript: Option<Option<Transcript>>,
    /// Showing the transcript instead of the article's text.
    pub show_transcript: bool,
    /// The Up Next queue; `None` until loaded, or from a server without one.
    pub queue: Option<Vec<Item>>,
    pub playback: Option<Playback>,
    /// Bumped for each episode loaded, so a late load of an episode since
    /// replaced (or stopped) is dropped.
    playback_generation: u64,
    /// Starts a player; tests swap it out.
    load_player: player::Loader,
    /// Connects to the system's media controls; tests swap it out.
    start_remote: fn() -> Option<Box<dyn Remote>>,
    remote: Option<Box<dyn Remote>>,
}

/// The episode playing (or loading).
pub struct Playback {
    pub item: Item,
    pub chapters: Vec<Chapter>,
    /// As of the last tick.
    pub state: PlayerState,
    pub video: bool,
    /// `None` while it loads.
    pub player: Option<Box<dyn Player>>,
    /// Where the progress slider is being dragged to.
    pub scrubbing: Option<f64>,
    /// The position last saved on the server.
    saved: u32,
}

impl Playback {
    /// An episode loading.
    pub fn new(item: Item, chapters: Vec<Chapter>, state: PlayerState, video: bool) -> Self {
        let mut playback = Self {
            item,
            chapters,
            state,
            video,
            player: None,
            scrubbing: None,
            saved: 0,
        };
        playback.saved = playback.position();
        playback
    }

    /// The chapter at the current position, if there are chapters.
    pub fn chapter(&self) -> Option<&Chapter> {
        self.chapters
            .iter()
            .filter(|c| !c.hidden)
            .take_while(|c| c.start <= self.state.position)
            .last()
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn position(&self) -> u32 {
        self.state.position.max(0.0) as u32
    }
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

    // Podcasts
    /// Plays the open article's episode (its video, with `true`), or pauses
    /// or resumes it if it is the one playing.
    Play(bool),
    PlayerLoaded(u64, api::Result<Loaded>),
    PlayerTick,
    PauseToggle,
    Stop,
    SeekBy(f64),
    SeekTo(f64),
    /// The progress slider is being dragged; `SeekTo` happens on release.
    Scrub(f64),
    ScrubDone,
    /// The next chapter, or (`false`) back one.
    Chapter(bool),
    NextSpeed,
    ChaptersLoaded(i64, api::Result<Chapters>),
    ToggleTranscript,
    TranscriptLoaded(i64, api::Result<Option<Transcript>>),
    PositionSaved(api::Result<()>),
    MediaKey(MediaKey),
    QueueLoaded(api::Result<Option<Vec<Item>>>),
    Queue(QueueChange),
    /// What the server kept of a new queue.
    QueueSet(api::Result<Vec<Item>>),

    ThemeMode(iced::theme::Mode),
}

impl App {
    // Not measured: reads the real user's config; tests use `boot_from`.
    #[cfg(not(tarpaulin_include))]
    pub fn boot() -> (Self, Task<Message>) {
        Self::boot_from(ClientConfig::default_path())
    }

    /// Starts on the reader if `config_path` and its token say this machine
    /// is paired, and on the pairing screen if not.
    fn boot_from(config_path: Option<std::path::PathBuf>) -> (Self, Task<Message>) {
        let (config, session) = config::load_session(config_path.as_deref());
        let mut app = Self {
            screen: Screen::Setup(Setup::default()),
            config,
            config_path,
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
        Task::batch([identity, load_feeds(&client), load_queue(&client), items])
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
        let (keys, playing) = match &self.screen {
            Screen::Reader(r) => (
                Subscription::batch([
                    keyboard::listen().filter_map(on_key),
                    Subscription::run(controls::keys).map(Message::MediaKey),
                ]),
                r.playback.is_some(),
            ),
            Screen::Setup(_) => (Subscription::none(), false),
        };
        // While playing, follow the player about once a second.
        let tick = if playing {
            iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::PlayerTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([mode, keys, tick])
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
                    setup.client = Some(client.clone());
                    Task::perform(
                        client.pair_start(setup.device.trim().to_owned()),
                        Message::PairStarted,
                    )
                }
                Message::PairStarted(Ok(start)) => {
                    // A late answer for a pairing that was given up on.
                    let Some(client) = setup.client.clone() else {
                        return Task::none();
                    };
                    setup.code = Some(start.code);
                    Task::perform(client.pair_wait(start.poll_id), Message::PairDone)
                }
                Message::PairStarted(Err(e)) | Message::PairDone(Err(e)) => {
                    setup.busy = false;
                    setup.code = None;
                    setup.error = Some(e);
                    Task::none()
                }
                Message::PairDone(Ok(approved)) => {
                    let Some(client) = setup.client.take() else {
                        return Task::none();
                    };
                    let session = Session {
                        server: client.server().to_owned(),
                        token: approved.token,
                    };
                    if let Err(e) =
                        config::save_session(&self.config, self.config_path.as_deref(), &session)
                    {
                        setup.busy = false;
                        setup.error = Some(format!("Paired, but could not save the token: {e:#}"));
                        return Task::none();
                    }
                    self.config.server = Some(session.server);
                    self.open_reader(client.with_token(session.token))
                }
                _ => Task::none(),
            },
            Screen::Reader(reader) => {
                if let Message::SignOut = message {
                    let _ = reader.stop();
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
    pub(crate) fn new(client: Client) -> Self {
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
            open_url: open_in_browser,
            chapters: Vec::new(),
            transcript: None,
            show_transcript: false,
            queue: None,
            playback: None,
            playback_generation: 0,
            load_player: player::load,
            start_remote: controls::start,
            remote: None,
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
            Source::UpNext => {}
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
        if self.showing_queue() {
            let queue = self.client.clone().queue();
            return Task::perform(
                async move {
                    Ok(ItemPage {
                        items: queue.await?.unwrap_or_default(),
                        next_cursor: None,
                        next_offset: None,
                    })
                },
                move |r| Message::ItemsLoaded(generation, false, r),
            );
        }
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
            Source::UpNext => "Up Next".into(),
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
        // As on the server: read items leave Up Next.
        if flag == Flag::Read && value {
            if let Some(queue) = self.queue.as_mut() {
                queue.retain(|i| i.id != id);
            }
            if self.showing_queue() {
                self.items.retain(|i| i.id != id);
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
        self.chapters.clear();
        self.transcript = None;
        self.show_transcript = false;
        let media = self.article.as_ref().and_then(|i| i.media.as_ref());
        let chapters = if media.is_some_and(|m| m.chapters.is_some()) {
            load_chapters(&self.client, id)
        } else {
            Task::none()
        };
        // An episode isn't read until it is played to the end.
        let unread = media.is_none() && self.item_mut(id).is_some_and(|i| !i.read);
        if unread {
            Task::batch([fetch, chapters, self.set_flag(id, Flag::Read, true)])
        } else {
            Task::batch([fetch, chapters])
        }
    }

    /// Sets an item's saved position here, in the list and the article.
    fn set_position_here(&mut self, id: i64, position: u32) {
        for item in self
            .items
            .iter_mut()
            .chain(self.article.iter_mut())
            .chain(self.queue.iter_mut().flatten())
            .filter(|i| i.id == id)
        {
            item.position = position;
        }
    }

    fn save_position(&mut self, id: i64, position: u32) -> Task<Message> {
        self.set_position_here(id, position);
        Task::perform(
            self.client.clone().set_position(id, position),
            Message::PositionSaved,
        )
    }

    /// `Play`: pauses or resumes the episode playing, or starts the open
    /// article's (its video, with `video`).
    fn play(&mut self, video: bool) -> Task<Message> {
        let article = self.article.clone().filter(|i| i.media.is_some());
        let same = |p: &Playback| article.as_ref().is_none_or(|a| a.id == p.item.id);
        if self
            .playback
            .as_ref()
            .is_some_and(|p| same(p) && p.video == video)
        {
            return self.update(Message::PauseToggle);
        }
        let Some(item) = article else {
            return Task::none();
        };
        self.start(item, video)
    }

    /// Starts an episode (its video, with `video`), resuming where it was
    /// left, after stopping whatever plays.
    fn start(&mut self, item: Item, video: bool) -> Task<Message> {
        let media = item.media.clone().unwrap_or_default();
        let enclosure = if video {
            media.video()
        } else {
            media.audio().or_else(|| media.video())
        };
        let Some(enclosure) = enclosure.cloned() else {
            return Task::none();
        };
        let stopped = self.stop();
        // Stopping may have just saved a newer position (switching between
        // an episode's audio and video).
        let position = self
            .items
            .iter()
            .chain(self.article.iter())
            .chain(self.queue.iter().flatten())
            .find(|i| i.id == item.id)
            .map_or(item.position, |i| i.position);
        // Played to the end before: start again.
        let start = match media.duration {
            Some(d) if f64::from(position) >= f64::from(d) - NEAR_END => 0,
            _ => position,
        };
        // The open article's chapters are loaded already; another
        // episode's (from Up Next) are fetched.
        let open = self.selected == Some(item.id);
        let fetch_chapters = if !open && media.chapters.is_some() {
            load_chapters(&self.client, item.id)
        } else {
            Task::none()
        };
        let chapters = if open {
            self.chapters.clone()
        } else {
            Vec::new()
        };
        if self.remote.is_none() {
            self.remote = (self.start_remote)();
        }
        if let Some(remote) = self.remote.as_mut() {
            let feed = self
                .feeds
                .iter()
                .find(|f| f.id == item.feed_id)
                .map_or(item.feed_id.as_str(), |f| f.title.as_str());
            let image = media.image.as_deref().or_else(|| {
                self.feeds
                    .iter()
                    .find(|f| f.id == item.feed_id)
                    .and_then(|f| f.image.as_deref())
            });
            remote.now_playing(&item.title, feed, image, media.duration.map(f64::from));
        }
        self.playback_generation += 1;
        let generation = self.playback_generation;
        let state = PlayerState {
            position: f64::from(start),
            duration: media.duration.map(f64::from),
            paused: false,
            speed: 1.0,
            ended: false,
        };
        self.playback = Some(Playback::new(item, chapters, state, video));
        let load = self.load_player;
        let url = enclosure.url;
        let loading = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || load(&url, start, video).map(Loaded::new))
                    .await
                    .map_err(|e| e.to_string())?
            },
            move |r| Message::PlayerLoaded(generation, r),
        );
        Task::batch([stopped, loading, fetch_chapters])
    }

    /// Stops playing. Played to (near) the end, the episode is marked read
    /// and its position forgotten; otherwise the position is saved.
    fn stop(&mut self) -> Task<Message> {
        let Some(mut playback) = self.playback.take() else {
            return Task::none();
        };
        if let Some(state) = playback.player.as_mut().and_then(|p| p.state()) {
            playback.state = state;
        }
        // Dropping the player stops it.
        drop(playback.player.take());
        if let Some(remote) = self.remote.as_mut() {
            remote.playback(None);
        }
        self.finish(&playback)
    }

    fn finish(&mut self, playback: &Playback) -> Task<Message> {
        let state = playback.state;
        let near_end = state
            .duration
            .is_some_and(|d| state.position >= d - NEAR_END);
        let id = playback.item.id;
        if state.ended || near_end {
            let read = self
                .items
                .iter()
                .find(|i| i.id == id)
                .map_or(playback.item.read, |i| i.read);
            let saved = self.save_position(id, 0);
            if !read {
                Task::batch([saved, self.set_flag(id, Flag::Read, true)])
            } else if self.is_queued(id) {
                // Played again: marking it read, which takes it out of Up
                // Next, changed nothing.
                let ids = self.queue_ids().into_iter().filter(|&q| q != id).collect();
                Task::batch([saved, self.set_queue(ids)])
            } else {
                saved
            }
        } else {
            self.save_position(id, playback.position())
        }
    }

    /// The item list is Up Next (not a search from it).
    pub fn showing_queue(&self) -> bool {
        self.source == Source::UpNext && self.search.trim().is_empty()
    }

    pub fn is_queued(&self, id: i64) -> bool {
        self.queue.iter().flatten().any(|i| i.id == id)
    }

    fn queue_ids(&self) -> Vec<i64> {
        self.queue.iter().flatten().map(|i| i.id).collect()
    }

    /// Replaces Up Next: reordered here at once, then as the server keeps
    /// it.
    fn set_queue(&mut self, ids: Vec<i64>) -> Task<Message> {
        if let Some(mut known) = self.queue.take() {
            known.extend(self.article.clone());
            let queue: Vec<Item> = ids
                .iter()
                .filter_map(|id| known.iter().find(|i| i.id == *id).cloned())
                .collect();
            if self.showing_queue() {
                self.items.clone_from(&queue);
            }
            self.queue = Some(queue);
        }
        Task::perform(self.client.clone().set_queue(ids), Message::QueueSet)
    }

    /// `Queue`: changes Up Next for the open article's episode.
    fn change_queue(&mut self, change: QueueChange) -> Task<Message> {
        let Some(id) = self
            .article
            .as_ref()
            .filter(|i| i.media.is_some())
            .map(|i| i.id)
        else {
            return Task::none();
        };
        if self.queue.is_none() {
            self.error = Some("This server has no Up Next; upgrade pressboxd.".into());
            return Task::none();
        }
        let mut ids = self.queue_ids();
        let at = ids.iter().position(|&q| q == id);
        match (change, at) {
            (QueueChange::Toggle, Some(at)) => {
                ids.remove(at);
            }
            (QueueChange::Toggle, None) => ids.push(id),
            (QueueChange::PlayNext, _) => {
                ids.retain(|&q| q != id);
                ids.insert(0, id);
            }
            (QueueChange::Move(by), Some(at)) => match at.checked_add_signed(by) {
                Some(to) if to < ids.len() => ids.swap(at, to),
                _ => return Task::none(),
            },
            (QueueChange::Move(_), None) => return Task::none(),
        }
        self.set_queue(ids)
    }

    /// `PlayerTick`: follows the player, saves the position now and then,
    /// and notices the end.
    fn tick(&mut self) -> Task<Message> {
        let Some(playback) = self.playback.as_mut() else {
            return Task::none();
        };
        let Some(player) = playback.player.as_mut() else {
            // Still loading.
            return Task::none();
        };
        match player.state() {
            Some(state) if !state.ended => {
                playback.state = state;
                if let Some(remote) = self.remote.as_mut() {
                    remote.playback(Some((state.paused, state.position)));
                }
                let position = playback.position();
                let moved = position.abs_diff(playback.saved) >= SAVE_EVERY
                    || (state.paused && position != playback.saved);
                if moved {
                    playback.saved = position;
                    let id = playback.item.id;
                    self.save_position(id, position)
                } else {
                    Task::none()
                }
            }
            Some(state) => {
                playback.state = state;
                let stopped = self.stop();
                // Up Next: carry on with the next episode.
                match self.queue.iter().flatten().next().cloned() {
                    Some(next) => Task::batch([stopped, self.start(next, false)]),
                    None => stopped,
                }
            }
            None => {
                self.error = Some("Playback stopped: the episode couldn't be played.".into());
                self.stop()
            }
        }
    }

    /// The loaded player, if one is playing.
    fn player(&mut self) -> Option<(&mut Box<dyn Player>, &mut PlayerState)> {
        let playback = self.playback.as_mut()?;
        Some((playback.player.as_mut()?, &mut playback.state))
    }

    fn seek_to(&mut self, seconds: f64) {
        if let Some((player, state)) = self.player() {
            let seconds = match state.duration {
                Some(d) => seconds.clamp(0.0, d),
                None => seconds.max(0.0),
            };
            player.seek(seconds);
            state.position = seconds;
            state.ended = false;
        }
    }

    /// The next chapter's start, or (`next` false) this one's, or the one
    /// before near its start.
    fn chapter_start(&self, next: bool) -> Option<f64> {
        let playback = self.playback.as_ref()?;
        let at = playback.state.position;
        let mut starts = playback
            .chapters
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| c.start);
        if next {
            starts.find(|s| *s > at + 0.5)
        } else {
            Some(starts.rfind(|s| *s < at - 3.0).unwrap_or(0.0))
        }
    }

    fn playback_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Play(video) => self.play(video),
            Message::PlayerLoaded(generation, result) => {
                if generation != self.playback_generation {
                    return Task::none();
                }
                match result.map(|l| l.take()) {
                    Ok(Some(player)) => {
                        if let Some(p) = self.playback.as_mut() {
                            p.player = Some(player);
                        }
                        Task::none()
                    }
                    Ok(None) => Task::none(),
                    Err(e) => {
                        self.playback = None;
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::PlayerTick => self.tick(),
            Message::PauseToggle => {
                if let Some((player, state)) = self.player() {
                    state.paused = !state.paused;
                    player.set_paused(state.paused);
                }
                Task::none()
            }
            Message::Stop => self.stop(),
            Message::SeekBy(by) => {
                let at = self.playback.as_ref().map(|p| p.state.position);
                if let Some(at) = at {
                    self.seek_to(at + by);
                }
                Task::none()
            }
            Message::SeekTo(to) => {
                self.seek_to(to);
                Task::none()
            }
            Message::Scrub(to) => {
                if let Some(p) = self.playback.as_mut() {
                    p.scrubbing = Some(to);
                }
                Task::none()
            }
            Message::ScrubDone => {
                if let Some(to) = self.playback.as_mut().and_then(|p| p.scrubbing.take()) {
                    self.seek_to(to);
                }
                Task::none()
            }
            Message::Chapter(next) => {
                if let Some(to) = self.chapter_start(next) {
                    self.seek_to(to);
                }
                Task::none()
            }
            Message::NextSpeed => {
                if let Some((player, state)) = self.player() {
                    let i = SPEEDS.iter().position(|s| (s - state.speed).abs() < 0.01);
                    state.speed = SPEEDS[i.map_or(0, |i| (i + 1) % SPEEDS.len())];
                    player.set_speed(state.speed);
                }
                Task::none()
            }
            Message::ChaptersLoaded(id, Ok(chapters)) => {
                if self.selected == Some(id) {
                    self.chapters.clone_from(&chapters.chapters);
                }
                if let Some(p) = self.playback.as_mut().filter(|p| p.item.id == id) {
                    p.chapters = chapters.chapters;
                }
                Task::none()
            }
            Message::ToggleTranscript => {
                let Some(id) = self.selected else {
                    return Task::none();
                };
                self.show_transcript = !self.show_transcript;
                if self.show_transcript && self.transcript.is_none() {
                    return Task::perform(self.client.clone().transcript(id), move |r| {
                        Message::TranscriptLoaded(id, r)
                    });
                }
                Task::none()
            }
            Message::TranscriptLoaded(id, Ok(transcript)) => {
                if self.selected == Some(id) {
                    self.transcript = Some(transcript);
                }
                Task::none()
            }
            Message::ChaptersLoaded(_, Err(e))
            | Message::TranscriptLoaded(_, Err(e))
            | Message::PositionSaved(Err(e)) => {
                self.error = Some(e);
                Task::none()
            }
            Message::QueueLoaded(Ok(queue)) => {
                self.queue = queue;
                Task::none()
            }
            Message::Queue(change) => self.change_queue(change),
            Message::QueueSet(Ok(items)) => {
                if self.showing_queue() {
                    self.items.clone_from(&items);
                }
                self.queue = Some(items);
                Task::none()
            }
            Message::QueueLoaded(Err(e)) => {
                self.error = Some(e);
                Task::none()
            }
            Message::QueueSet(Err(e)) => {
                self.error = Some(e);
                // Back to what the server has.
                load_queue(&self.client)
            }
            Message::MediaKey(key) => {
                let paused = self.playback.as_ref().map(|p| p.state.paused);
                match key {
                    MediaKey::Play if paused == Some(true) => self.update(Message::PauseToggle),
                    MediaKey::Pause if paused == Some(false) => self.update(Message::PauseToggle),
                    MediaKey::Toggle => self.update(Message::PauseToggle),
                    MediaKey::SeekBy(by) => self.update(Message::SeekBy(by)),
                    MediaKey::SeekTo(to) => self.update(Message::SeekTo(to)),
                    MediaKey::Stop => self.stop(),
                    MediaKey::Play | MediaKey::Pause => Task::none(),
                }
            }
            _ => Task::none(),
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
                    && let Err(e) = (self.open_url)(&url)
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
                    Source::Starred | Source::UpNext => Vec::new(),
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
                Task::batch([
                    load_feeds(&self.client),
                    load_queue(&self.client),
                    self.reload(),
                ])
            }
            Message::Refresh => {
                self.status = Some("Refreshing feeds…".into());
                Task::perform(self.client.clone().refresh(), Message::Refreshed)
            }
            Message::Refreshed(Ok(())) => {
                self.status = None;
                Task::batch([
                    load_feeds(&self.client),
                    load_queue(&self.client),
                    self.reload(),
                ])
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
            message => self.playback_message(message),
        }
    }
}

// Not measured: launches the user's browser. Tests give the reader their
// own `open_url`.
#[cfg(not(tarpaulin_include))]
fn open_in_browser(url: &str) -> std::io::Result<()> {
    open::that_detached(url)
}

fn load_feeds(client: &Client) -> Task<Message> {
    Task::perform(client.clone().feeds(), Message::FeedsLoaded)
}

fn load_chapters(client: &Client, id: i64) -> Task<Message> {
    Task::perform(client.clone().chapters(id), move |r| {
        Message::ChaptersLoaded(id, r)
    })
}

fn load_queue(client: &Client) -> Task<Message> {
    Task::perform(client.clone().queue(), Message::QueueLoaded)
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
        Key::Character("p") | Key::Named(Named::Space) => Some(Message::Play(false)),
        Key::Character("V") => Some(Message::Play(true)),
        Key::Character("x") => Some(Message::Stop),
        Key::Character(",") => Some(Message::SeekBy(-controls::BACK)),
        Key::Character(".") => Some(Message::SeekBy(controls::FORWARD)),
        Key::Character("[") => Some(Message::Chapter(false)),
        Key::Character("]") => Some(Message::Chapter(true)),
        Key::Character("t") => Some(Message::ToggleTranscript),
        Key::Character("a") => Some(Message::Queue(QueueChange::Toggle)),
        Key::Character("n") => Some(Message::Queue(QueueChange::PlayNext)),
        Key::Character("J") => Some(Message::Queue(QueueChange::Move(1))),
        Key::Character("K") => Some(Message::Queue(QueueChange::Move(-1))),
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
    use iced::Color;
    use pressbox_api::{Me, Version};

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
            image: None,
            medium: None,
            episodes: 0,
            unread_episodes: 0,
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
            media: None,
            position: 0,
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

    /// An app on the pairing screen whose files live in a temporary
    /// directory, so nothing touches the real config.
    fn setup_app(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("pressbox-iced-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App {
            config: ClientConfig {
                server: None,
                token_file: Some(dir.join("token")),
            },
            config_path: Some(dir.join("client.json")),
            screen: Screen::Setup(Setup::default()),
            dark: true,
        };
        app.screen = Screen::Setup(app.new_setup());
        (app, dir)
    }

    fn setup(app: &App) -> &Setup {
        match &app.screen {
            Screen::Setup(s) => s,
            Screen::Reader(_) => panic!("expected the setup screen"),
        }
    }

    #[test]
    fn pairing_saves_the_session_and_opens_the_reader() {
        let (mut app, dir) = setup_app("pair");
        assert!(!setup(&app).device.is_empty(), "defaults to the host name");
        let _ = app.update(Message::DeviceChanged(String::new()));
        let _ = app.update(Message::Pair);
        assert_eq!(
            setup(&app).error.as_deref(),
            Some("Enter the server address and a device name.")
        );

        let _ = app.update(Message::ServerChanged("rss.example/".into()));
        let _ = app.update(Message::DeviceChanged("laptop".into()));
        let _ = app.update(Message::Pair);
        assert!(setup(&app).busy && setup(&app).error.is_none());
        let _ = app.update(Message::PairStarted(Ok(PairStart {
            code: "K7W-4QX".into(),
            poll_id: "p".into(),
            expires_in: 600,
        })));
        assert_eq!(setup(&app).code.as_deref(), Some("K7W-4QX"));
        assert_eq!(app.title(), "pressbox");

        let _ = app.update(Message::PairDone(Ok(PairApproved {
            token: "secret".into(),
            name: "laptop".into(),
        })));
        let Screen::Reader(reader) = &app.screen else {
            panic!("expected the reader");
        };
        assert_eq!(reader.client.server(), "https://rss.example");
        assert!(reader.loading, "the first page is on its way");
        assert_eq!(app.config.server.as_deref(), Some("https://rss.example"));
        assert_eq!(app.config.token().as_deref(), Some("secret"));
        let saved = ClientConfig::load(&dir.join("client.json")).unwrap();
        assert_eq!(saved.server.as_deref(), Some("https://rss.example"));

        if let Screen::Reader(reader) = &mut app.screen {
            reader.feeds = vec![feed("a", 4, &[])];
        }
        assert_eq!(app.title(), "pressbox (4)");
        if let Screen::Reader(reader) = &mut app.screen {
            reader.feeds[0].unread = 0;
        }
        assert_eq!(app.title(), "pressbox", "no count when all is read");

        // Signing out forgets the token and goes back to pairing, with the
        // server filled in.
        let _ = app.update(Message::SignOut);
        assert_eq!(setup(&app).server, "https://rss.example");
        assert!(app.config.token().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pairing_failures_are_shown() {
        let (mut app, dir) = setup_app("fail");
        let _ = app.update(Message::ServerChanged("https://rss.example".into()));
        let _ = app.update(Message::Pair);
        let _ = app.update(Message::PairStarted(Err("could not connect".into())));
        let s = setup(&app);
        assert!(!s.busy && s.code.is_none());
        assert_eq!(s.error.as_deref(), Some("could not connect"));

        let _ = app.update(Message::PairDone(Err("denied".into())));
        assert_eq!(setup(&app).error.as_deref(), Some("denied"));

        // The token can't be written: paired, but say so and stay here.
        app.config.token_file = Some(std::path::PathBuf::from("/proc/nope/token"));
        let _ = app.update(Message::PairDone(Ok(PairApproved {
            token: "secret".into(),
            name: "laptop".into(),
        })));
        let error = setup(&app).error.clone().unwrap();
        assert!(
            error.starts_with("Paired, but could not save the token"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn theme_follows_the_system() {
        let (mut app, _) = setup_app("theme");
        let _ = app.update(Message::ThemeMode(iced::theme::Mode::Light));
        assert!(!app.dark);
        let light = app.theme();
        let _ = app.update(Message::ThemeMode(iced::theme::Mode::Dark));
        assert!(app.dark);
        assert_ne!(app.theme().palette(), light.palette());
    }

    #[test]
    fn errors_and_refreshes() {
        let mut r = reader();
        for message in [
            Message::FeedsLoaded(Err("a".into())),
            Message::ItemLoaded(Err("b".into())),
            Message::Refreshed(Err("c".into())),
            Message::Approved(Err("d".into())),
            Message::MarkedAllRead(Err("e".into())),
            Message::Identity(Err("f".into())),
        ] {
            let _ = r.update(message);
        }
        assert_eq!(r.error.as_deref(), Some("f"));
        let _ = r.update(Message::Refresh);
        assert_eq!(r.status.as_deref(), Some("Refreshing feeds…"));
        let _ = r.update(Message::Refreshed(Ok(())));
        assert!(r.status.is_none() && r.loading);
        let _ = r.update(Message::FeedsLoaded(Ok(vec![feed("a", 1, &[])])));
        assert_eq!(r.feeds.len(), 1);
        let _ = r.update(Message::Identity(Ok((
            Me {
                user: "chris".into(),
                admin: true,
                device: None,
            },
            Version {
                version: "0.1.0".into(),
                api: 1,
            },
        ))));
        assert_eq!(r.me.as_ref().unwrap().user, "chris");

        // Mark all read covers the feeds the source shows with unread items.
        r.feeds = vec![
            feed("a", 1, &["t"]),
            feed("b", 0, &["t"]),
            feed("c", 2, &[]),
        ];
        for source in [Source::Starred, Source::Tag("none".into())] {
            r.source = source;
            let _ = r.update(Message::MarkAllRead);
        }
        let _ = r.update(Message::Approve);
        let _ = r.update(Message::SearchChanged("rust".into()));
        let _ = r.update(Message::SearchSubmit);
        assert_eq!(r.query().q.as_deref(), Some("rust"));
        let _ = r.update(Message::MarkedAllRead(Ok(3)));
        assert_eq!(r.status.as_deref(), Some("Marked 3 items as read"));
        let _ = r.update(Message::Toggle(Flag::Read));
        let _ = r.update(Message::OpenInBrowser);
        assert!(r.article.is_none(), "nothing open, nothing toggled");
    }

    /// Runs `task` until it has produced `n` messages (or fails the test
    /// after five seconds), as the iced runtime would. Other actions (asking
    /// the system for its theme, say) are left unanswered.
    async fn run(task: Task<Message>, n: usize) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            assert_eq!(n, 0, "the task does nothing");
            return Vec::new();
        };
        let messages = stream
            .filter_map(|action| async move {
                match action {
                    iced_runtime::Action::Output(message) => Some(message),
                    _ => None,
                }
            })
            .take(n)
            .collect();
        tokio::time::timeout(std::time::Duration::from_secs(5), messages)
            .await
            .expect("the task's messages")
    }

    const FEEDS: &str = r#"[
        {"id":"a","title":"A","url":"https://a.example/","site_url":null,"tags":["t"],
         "unread":1,"total":1,"last_fetched":null,"last_error":null,"archived":false},
        {"id":"b","title":"B","url":"https://b.example/","site_url":null,"tags":[],
         "unread":2,"total":2,"last_fetched":null,"last_error":null,"archived":false}]"#;
    const ONE_PAGE: &str = r#"{"items":[{"id":7,"feed_id":"a","title":"Seven","url":"https://a.example/7",
        "author":null,"summary":"<p>s</p>","published":0,"read":false,"starred":false}],
        "next_cursor":"c7"}"#;
    const ITEM: &str = r#"{"id":7,"feed_id":"a","title":"Seven","url":"https://a.example/7",
        "author":null,"summary":"<p>s</p>","content":"<p>The whole of it.</p>","published":0,
        "read":false,"starred":false}"#;

    /// A server with everything the reader asks for.
    fn reader_server() -> (String, std::sync::mpsc::Receiver<String>) {
        crate::test_server::routed(vec![
            (
                "GET /api/v1/me",
                200,
                r#"{"user":"chris","admin":true,"device":"pc"}"#,
            ),
            ("GET /api/v1/version", 200, r#"{"version":"0.1.0","api":1}"#),
            ("GET /api/v1/feeds", 200, FEEDS),
            ("GET /api/v1/queue", 200, r#"{"items":[]}"#),
            ("GET /api/v1/items/7", 200, ITEM),
            ("GET /api/v1/items", 200, ONE_PAGE),
            ("PUT /api/v1/items/7/", 204, ""),
            ("POST /api/v1/feeds/", 200, r#"{"updated":1}"#),
            ("POST /api/v1/refresh", 202, ""),
            (
                "POST /api/v1/pairings/",
                200,
                r#"{"name":"phone","user":"chris"}"#,
            ),
            (
                "POST /api/v1/pair",
                201,
                r#"{"code":"K7W-4QX","poll_id":"p1","expires_in":600}"#,
            ),
            (
                "GET /api/v1/pair/p1",
                200,
                r#"{"token":"new-token","name":"laptop"}"#,
            ),
        ])
    }

    /// Feeds each message back in, as the runtime does, and returns the
    /// tasks that answers start.
    fn answer(app: &mut App, messages: Vec<Message>) -> Vec<Task<Message>> {
        messages.into_iter().map(|m| app.update(m)).collect()
    }

    fn reading(app: &App) -> &Reader {
        match &app.screen {
            Screen::Reader(r) => r,
            Screen::Setup(_) => panic!("expected the reader"),
        }
    }

    #[tokio::test]
    async fn boots_into_the_reader_when_paired() {
        let (base, _requests) = reader_server();
        let dir = std::env::temp_dir().join(format!("pressbox-iced-boot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        let token = dir.join("token");
        std::fs::write(&token, "secret\n").unwrap();
        std::fs::write(
            &path,
            format!(r#"{{"server":"{base}","tokenFile":"{}"}}"#, token.display()),
        )
        .unwrap();

        let (mut app, task) = App::boot_from(Some(path.clone()));
        // Identity, feeds, Up Next and items, in whatever order they come.
        let messages = run(task, 4).await;
        for wanted in [
            "Identity(Ok",
            "FeedsLoaded(Ok",
            "QueueLoaded(Ok",
            "ItemsLoaded(",
        ] {
            assert!(
                messages
                    .iter()
                    .any(|m| format!("{m:?}").starts_with(wanted)),
                "{wanted} missing: {messages:?}"
            );
        }
        let _ = answer(&mut app, messages);
        let reader = reading(&app);
        assert_eq!(reader.me.as_ref().map(|m| m.user.as_str()), Some("chris"));
        assert_eq!(reader.queue, Some(vec![]), "the server has Up Next");
        assert_eq!((reader.feeds.len(), reader.items.len()), (2, 1));
        assert_eq!(app.title(), "pressbox (3)");
        let _ = app.subscription();

        // Without a token: the pairing screen, with the server filled in.
        std::fs::remove_file(&token).unwrap();
        let (app, _) = App::boot_from(Some(path));
        assert!(matches!(&app.screen, Screen::Setup(s) if s.server == base));
        let _ = app.subscription();
        let (app, _) = App::boot_from(None);
        assert!(matches!(&app.screen, Screen::Setup(s) if s.server.is_empty()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn pairs_through_the_server() {
        let (base, requests) = reader_server();
        let (mut app, dir) = setup_app("tasks");
        let _ = app.update(Message::ServerChanged("http://bad host".into()));
        let _ = app.update(Message::Pair);
        assert!(
            setup(&app)
                .error
                .as_deref()
                .unwrap()
                .contains("not a server address")
        );

        // Answers for a pairing that isn't under way are ignored.
        let started = PairStart {
            code: "X".into(),
            poll_id: "x".into(),
            expires_in: 1,
        };
        let _ = app.update(Message::PairStarted(Ok(started)));
        assert!(setup(&app).code.is_none());
        let approved = PairApproved {
            token: "t".into(),
            name: "n".into(),
        };
        let _ = app.update(Message::PairDone(Ok(approved)));
        assert!(matches!(app.screen, Screen::Setup(_)));
        let _ = app.update(Message::Refresh);
        assert!(
            setup(&app).error.is_some(),
            "reader messages change nothing here"
        );

        let _ = app.update(Message::ServerChanged(base.clone()));
        let started = run(app.update(Message::Pair), 1).await;
        let done = run(answer(&mut app, started).remove(0), 1).await;
        assert_eq!(setup(&app).code.as_deref(), Some("K7W-4QX"));
        let _ = answer(&mut app, done);
        assert_eq!(reading(&app).client.server(), base);
        assert_eq!(app.config.token().as_deref(), Some("new-token"));
        let lines: Vec<String> = requests.try_iter().collect();
        assert_eq!(lines, ["POST /api/v1/pair", "GET /api/v1/pair/p1"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_readers_tasks_talk_to_the_server() {
        let (base, requests) = reader_server();
        let mut r = Reader::new(Client::new(&base, Some("t".into())).unwrap());
        let loaded = run(r.reload(), 1).await;
        let _ = feed_reader(&mut r, loaded);
        let feeds = run(load_feeds(&r.client), 1).await;
        let _ = feed_reader(&mut r, feeds);
        assert_eq!((r.items.len(), r.next_cursor.as_deref()), (1, Some("c7")));

        // Opening an unread item fetches it and marks it read.
        let opened = run(r.update(Message::OpenItem(7)), 2).await;
        let _ = feed_reader(&mut r, opened);
        let article = r.article.as_ref().unwrap();
        assert!(article.read && r.article_text.contains("The whole of it."));
        assert_eq!(r.feeds[0].unread, 0);
        // A read item is only fetched.
        assert_eq!(run(r.update(Message::OpenItem(7)), 1).await.len(), 1);

        let starred = run(r.update(Message::Toggle(Flag::Starred)), 1).await;
        assert!(matches!(
            starred[..],
            [Message::Flagged(7, Flag::Starred, true, Ok(()))]
        ));
        let _ = feed_reader(&mut r, starred);
        let _ = r.update(Message::Toggle(Flag::Read));
        assert!(!r.items[0].read, "marked unread again");

        // Reaching the end loads the next page.
        let more = run(r.update(Message::Next), 1).await;
        assert!(matches!(more[..], [Message::ItemsLoaded(_, true, Ok(_))]));

        // Mark all read covers every feed with unread items, or the one shown.
        r.feeds[0].unread = 1;
        let all = run(r.update(Message::MarkAllRead), 1).await;
        assert!(
            matches!(all[..], [Message::MarkedAllRead(Ok(2))]),
            "{all:?}"
        );
        r.source = Source::Feed("a".into());
        assert_eq!(r.source_title(), "A");
        let one = run(r.update(Message::MarkAllRead), 1).await;
        assert!(matches!(one[..], [Message::MarkedAllRead(Ok(1))]));

        let _ = r.update(Message::ApproveCodeChanged("k7w-4qx".into()));
        let approved = run(r.update(Message::Approve), 1).await;
        assert!(matches!(&approved[..], [Message::Approved(Ok(a))] if a.name == "phone"));
        let refreshed = run(r.update(Message::Refresh), 1).await;
        assert!(matches!(refreshed[..], [Message::Refreshed(Ok(()))]));
        assert_eq!(run(r.update(Message::Refreshed(Ok(()))), 3).await.len(), 3);

        // Up Next lists the server's queue; searching it searches everything.
        let queued = run(r.update(Message::Select(Source::UpNext)), 1).await;
        assert!(matches!(
            queued[..],
            [Message::ItemsLoaded(_, false, Ok(_))]
        ));
        assert_eq!(r.source_title(), "Up Next");
        r.search = "seven".into();
        let searched = run(r.reload(), 1).await;
        assert!(matches!(
            searched[..],
            [Message::ItemsLoaded(_, false, Ok(_))]
        ));

        let seen: Vec<String> = requests.try_iter().collect();
        for expected in [
            "PUT /api/v1/items/7/read",
            "PUT /api/v1/items/7/starred",
            "GET /api/v1/items?unread=true&limit=50&cursor=c7",
            "POST /api/v1/feeds/a/read-all",
            "POST /api/v1/feeds/b/read-all",
            "POST /api/v1/pairings/K7W-4QX/approve",
            "POST /api/v1/refresh",
            "GET /api/v1/queue",
            "GET /api/v1/items?q=seven&limit=50",
        ] {
            assert!(
                seen.iter().any(|s| s.starts_with(expected)),
                "{expected} in {seen:?}"
            );
        }
    }

    fn feed_reader(r: &mut Reader, messages: Vec<Message>) -> Vec<Task<Message>> {
        messages.into_iter().map(|m| r.update(m)).collect()
    }

    #[test]
    fn small_things() {
        let mut r = reader();
        r.items = vec![item(1, "a")];
        r.source = Source::All;
        assert_eq!(r.source_title(), "All items");
        let _ = r.update(Message::Flagged(1, Flag::Read, true, Ok(())));
        let _ = r.update(Message::Pair);
        assert!(
            r.error.is_none(),
            "messages for the other screen are ignored"
        );

        // A link that can't be opened says so.
        r.open_url = |_| Err(std::io::Error::other("no browser"));
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::OpenInBrowser);
        assert_eq!(
            r.error.as_deref(),
            Some("could not open https://a.example/1: no browser")
        );
        r.open_url = |_| Ok(());
        r.error = None;
        let _ = r.update(Message::OpenInBrowser);
        assert!(r.error.is_none());

        assert_eq!(theme::rgb(0x12_34_56), Color::from_rgb8(0x12, 0x34, 0x56));
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
        assert!(on_key(press(char("z"), Modifiers::empty())).is_none());
        assert!(matches!(
            on_key(press(Key::Named(Named::Escape), Modifiers::empty())),
            Some(Message::Dismiss)
        ));
        let release = keyboard::Event::KeyReleased {
            key: char("j"),
            modified_key: char("j"),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
        };
        assert!(on_key(release).is_none(), "only presses");
    }

    thread_local! {
        /// What the fake media controls were told.
        static REMOTE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    struct FakeRemote;

    impl Remote for FakeRemote {
        fn now_playing(&mut self, title: &str, feed: &str, image: Option<&str>, d: Option<f64>) {
            REMOTE.with(|r| {
                r.borrow_mut()
                    .push(format!("{title} / {feed} / {image:?} / {d:?}"))
            });
        }
        fn playback(&mut self, state: Option<(bool, f64)>) {
            REMOTE.with(|r| r.borrow_mut().push(format!("{state:?}")));
        }
    }

    fn remote() -> Vec<String> {
        REMOTE.with(|r| r.borrow().clone())
    }

    /// Item 1 is an episode (audio and video, chapters, a transcript, 1000s
    /// long, started at 100s); item 2 is a post.
    fn podcast_reader() -> (Reader, crate::player::tests::Fake) {
        use pressbox_api::{Enclosure, Media, TranscriptFile};
        REMOTE.with(|r| r.borrow_mut().clear());
        let mut r = reader();
        r.start_remote = || Some(Box::new(FakeRemote));
        r.load_player = |_, _, _| Err("tests load players by hand".into());
        let mut a = feed("a", 2, &[]);
        a.image = Some("https://a.example/art.jpg".into());
        r.feeds = vec![a];
        let enclosure = |url: &str, mime: &str| Enclosure {
            url: url.into(),
            mime: Some(mime.into()),
            ..Enclosure::default()
        };
        let mut episode = item(1, "a");
        episode.media = Some(Media {
            enclosures: vec![
                enclosure("https://a.example/1.mp3", "audio/mpeg"),
                enclosure("https://a.example/1.mp4", "video/mp4"),
            ],
            duration: Some(1000),
            chapters: Some("https://a.example/1.json".into()),
            transcripts: vec![TranscriptFile {
                url: "https://a.example/1.vtt".into(),
                mime: "text/vtt".into(),
                ..TranscriptFile::default()
            }],
            ..Media::default()
        });
        episode.position = 100;
        r.items = vec![episode, item(2, "a")];
        (r, crate::player::tests::Fake::default())
    }

    fn chapters() -> Chapters {
        let chapter = |start: f64, title: &str, hidden| Chapter {
            start,
            title: Some(title.into()),
            hidden,
            ..Chapter::default()
        };
        Chapters {
            chapters: vec![
                chapter(0.0, "Intro", false),
                chapter(60.0, "art", true),
                chapter(120.0, "Main", false),
                chapter(600.0, "End", false),
            ],
        }
    }

    /// Hands the reader its loaded player, as the load task would.
    fn loaded(r: &mut Reader, fake: &crate::player::tests::Fake) {
        let generation = r.playback_generation;
        let _ = r.update(Message::PlayerLoaded(
            generation,
            Ok(Loaded::new(fake.boxed())),
        ));
        fake.at(r.playback.as_ref().unwrap().state.position, false, false);
    }

    fn state(r: &Reader) -> PlayerState {
        r.playback.as_ref().unwrap().state
    }

    #[test]
    fn opening_an_episode_loads_chapters_and_leaves_it_unread() {
        let (mut r, _) = podcast_reader();
        let _ = r.update(Message::OpenItem(1));
        assert!(!r.items[0].read, "episodes are read once played through");
        assert_eq!(r.feeds[0].unread, 2);
        let _ = r.update(Message::ChaptersLoaded(1, Ok(chapters())));
        assert_eq!(r.chapters.len(), 4);
        let _ = r.update(Message::ChaptersLoaded(2, Ok(chapters())));
        let _ = r.update(Message::ChaptersLoaded(1, Err("502".into())));
        assert_eq!(r.error.as_deref(), Some("502"));

        // The transcript loads the first time it is shown.
        let _ = r.update(Message::ToggleTranscript);
        assert!(r.show_transcript && r.transcript.is_none());
        let _ = r.update(Message::TranscriptLoaded(1, Ok(None)));
        assert_eq!(r.transcript, Some(None));
        let _ = r.update(Message::TranscriptLoaded(2, Ok(None)));
        let _ = r.update(Message::ToggleTranscript);
        let _ = r.update(Message::ToggleTranscript);
        assert!(r.show_transcript);

        // A post: no chapters, read on opening, and a fresh transcript slate.
        let _ = r.update(Message::OpenItem(2));
        assert!(r.items[1].read && r.chapters.is_empty() && !r.show_transcript);
        r.selected = None;
        let _ = r.update(Message::ToggleTranscript);
        assert!(!r.show_transcript, "nothing open");
    }

    #[test]
    fn up_next_queues_reorders_and_plays_on() {
        let (mut r, fake) = podcast_reader();
        let mut three = r.items[0].clone();
        three.id = 3;
        three.title = "Item 3".into();
        three.position = 0;
        r.items.push(three);
        let ids = |r: &Reader| r.queue_ids();
        let mut one = r.items[0].clone();

        // A server without a queue says so.
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Queue(QueueChange::Toggle));
        assert!(r.error.take().unwrap().contains("no Up Next"));

        r.queue = Some(Vec::new());
        let _ = r.update(Message::Queue(QueueChange::Toggle));
        let _ = r.update(Message::OpenItem(2));
        let _ = r.update(Message::Queue(QueueChange::Toggle));
        assert_eq!(ids(&r), [1], "only episodes");
        let _ = r.update(Message::OpenItem(3));
        let _ = r.update(Message::Queue(QueueChange::PlayNext));
        assert_eq!(ids(&r), [3, 1]);

        // Listed as Up Next, it reorders in place.
        r.source = Source::UpNext;
        r.items = r.queue.clone().unwrap();
        let _ = r.update(Message::Queue(QueueChange::Move(1)));
        assert_eq!(ids(&r), [1, 3]);
        assert_eq!(r.items.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 3]);
        let _ = r.update(Message::Queue(QueueChange::Move(1)));
        assert_eq!(ids(&r), [1, 3], "already last");
        // What the server kept wins.
        let kept = r.queue.clone().unwrap();
        let _ = r.update(Message::QueueSet(Ok(kept)));
        let _ = r.update(Message::QueueSet(Err("offline".into())));
        assert_eq!(r.error.take().as_deref(), Some("offline"));

        // Played to the end, an episode leaves Up Next and the next starts.
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);
        fake.at(1000.0, false, true);
        let _ = r.update(Message::PlayerTick);
        assert_eq!(ids(&r), [3]);
        assert_eq!(r.items.iter().map(|i| i.id).collect::<Vec<_>>(), [3]);
        let p = r.playback.as_ref().unwrap();
        assert_eq!((p.item.id, p.state.position), (3, 0.0));

        // Stopping doesn't go on.
        loaded(&mut r, &fake);
        let _ = r.update(Message::Stop);
        assert!(r.playback.is_none());

        // Back in a list with both: moving one that isn't queued does
        // nothing; toggling takes it out.
        r.source = Source::All;
        one.read = true;
        r.items = vec![one, r.items[0].clone()];
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Queue(QueueChange::Move(1)));
        assert_eq!(ids(&r), [3]);
        let _ = r.update(Message::OpenItem(3));
        let _ = r.update(Message::Queue(QueueChange::Toggle));
        assert!(ids(&r).is_empty());

        // Finished again when already read, it is taken out by hand.
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Queue(QueueChange::Toggle));
        assert_eq!(ids(&r), [1]);
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);
        fake.at(1000.0, false, true);
        let _ = r.update(Message::PlayerTick);
        assert!(ids(&r).is_empty() && r.playback.is_none());

        let _ = r.update(Message::QueueLoaded(Err("offline".into())));
        assert_eq!(r.error.as_deref(), Some("offline"));
    }

    #[test]
    fn plays_resumes_and_saves_the_position() {
        let (mut r, fake) = podcast_reader();
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::ChaptersLoaded(1, Ok(chapters())));
        let _ = r.update(Message::Play(false));
        let p = r.playback.as_ref().unwrap();
        assert!(p.player.is_none(), "loading");
        assert_eq!((p.state.position, p.chapters.len()), (100.0, 4));
        assert_eq!(
            remote(),
            ["Item 1 / Feed a / Some(\"https://a.example/art.jpg\") / Some(1000.0)"]
        );
        let _ = r.update(Message::PlayerTick);
        let _ = r.update(Message::PauseToggle);
        assert!(!state(&r).paused, "nothing to pause while loading");

        // A late load for an earlier episode is dropped.
        let stale = Loaded::new(fake.boxed());
        let _ = r.update(Message::PlayerLoaded(0, Ok(stale)));
        assert!(r.playback.as_ref().unwrap().player.is_none());
        loaded(&mut r, &fake);
        let _ = r.update(Message::PlayerLoaded(
            r.playback_generation,
            Ok(Loaded::new(fake.boxed())),
        ));
        let taken = Loaded::new(fake.boxed());
        taken.take();
        let _ = r.update(Message::PlayerLoaded(r.playback_generation, Ok(taken)));

        fake.at(110.0, false, false);
        let _ = r.update(Message::PlayerTick);
        assert_eq!(state(&r).position, 110.0);
        assert_eq!(r.items[0].position, 100, "not saved yet");
        fake.at(130.0, false, false);
        let _ = r.update(Message::PlayerTick);
        assert_eq!(r.items[0].position, 130);
        assert_eq!(
            r.playback
                .as_ref()
                .unwrap()
                .chapter()
                .unwrap()
                .title
                .as_deref(),
            Some("Main")
        );
        assert_eq!(remote().last().unwrap(), "Some((false, 130.0))");

        // Pausing (Play on the same episode toggles) saves at the next tick.
        let _ = r.update(Message::Play(false));
        assert!(state(&r).paused);
        fake.at(131.0, true, false);
        let _ = r.update(Message::PlayerTick);
        assert_eq!(r.items[0].position, 131);
        let _ = r.update(Message::PlayerTick);

        // Stopping saves where it was; the episode stays unread.
        let _ = r.update(Message::Stop);
        assert!(r.playback.is_none());
        assert!(!r.items[0].read);
        assert_eq!(r.items[0].position, 131);
        assert_eq!(remote().last().unwrap(), "None");
        assert!(fake.calls().contains(&"stop".to_owned()));
        let _ = r.update(Message::Stop);
        let _ = r.update(Message::PlayerTick);
        let _ = r.update(Message::PositionSaved(Err("offline".into())));
        assert_eq!(r.error.as_deref(), Some("offline"));
        let _ = r.update(Message::PositionSaved(Ok(())));
    }

    #[test]
    fn seeking_chapters_speed_and_media_keys() {
        let (mut r, fake) = podcast_reader();
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::ChaptersLoaded(1, Ok(chapters())));
        // Nothing playing: nothing happens.
        for m in [
            Message::SeekBy(5.0),
            Message::SeekTo(5.0),
            Message::Scrub(5.0),
            Message::ScrubDone,
            Message::Chapter(true),
            Message::NextSpeed,
            Message::MediaKey(MediaKey::Play),
        ] {
            let _ = r.update(m);
        }
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);

        let _ = r.update(Message::SeekBy(30.0));
        assert_eq!(state(&r).position, 130.0);
        let _ = r.update(Message::SeekTo(5000.0));
        assert_eq!(state(&r).position, 1000.0, "not past the end");
        let _ = r.update(Message::SeekBy(-5000.0));
        assert_eq!(state(&r).position, 0.0);
        let _ = r.update(Message::Scrub(300.0));
        assert_eq!(state(&r).position, 0.0, "not until released");
        let _ = r.update(Message::ScrubDone);
        assert_eq!(state(&r).position, 300.0);
        let _ = r.update(Message::ScrubDone);

        let _ = r.update(Message::Chapter(true));
        assert_eq!(state(&r).position, 600.0, "hidden chapters are skipped");
        let _ = r.update(Message::Chapter(true));
        assert_eq!(state(&r).position, 600.0, "no more");
        let _ = r.update(Message::Chapter(false));
        assert_eq!(state(&r).position, 120.0, "near its start: the one before");
        let _ = r.update(Message::SeekTo(130.0));
        let _ = r.update(Message::Chapter(false));
        assert_eq!(state(&r).position, 120.0, "the start of this one");

        for expected in [1.25, 1.5, 1.75, 2.0, 0.75, 1.0] {
            let _ = r.update(Message::NextSpeed);
            assert_eq!(state(&r).speed, expected);
        }
        r.playback.as_mut().unwrap().state.speed = 1.1;
        let _ = r.update(Message::NextSpeed);
        assert_eq!(state(&r).speed, 1.0, "an odd speed starts over");

        for (key, paused) in [
            (MediaKey::Pause, true),
            (MediaKey::Pause, true),
            (MediaKey::Play, false),
            (MediaKey::Play, false),
            (MediaKey::Toggle, true),
        ] {
            let _ = r.update(Message::MediaKey(key));
            assert_eq!(state(&r).paused, paused, "{key:?}");
        }
        let _ = r.update(Message::MediaKey(MediaKey::SeekTo(50.0)));
        let _ = r.update(Message::MediaKey(MediaKey::SeekBy(10.0)));
        assert_eq!(state(&r).position, 60.0);
        let _ = r.update(Message::MediaKey(MediaKey::Stop));
        assert!(r.playback.is_none());
        assert!(fake.calls().contains(&"speed 1.25".to_owned()));
    }

    #[test]
    fn finishing_marks_read_and_switching_to_video_resumes() {
        let (mut r, fake) = podcast_reader();
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);
        fake.at(400.0, false, false);
        let _ = r.update(Message::PlayerTick);

        // The video starts where the audio was.
        let _ = r.update(Message::Play(true));
        let p = r.playback.as_ref().unwrap();
        assert!(p.video && p.player.is_none());
        assert_eq!(p.state.position, 400.0);
        loaded(&mut r, &fake);

        // Played to the end: read, and the position forgotten.
        fake.at(1000.0, false, true);
        let _ = r.update(Message::PlayerTick);
        assert!(r.playback.is_none());
        assert!(r.items[0].read && r.article.as_ref().unwrap().read);
        assert_eq!((r.items[0].position, r.feeds[0].unread), (0, 1));

        // Played to the end before: from the top. Stopped near the end:
        // finished, and already read.
        r.items[0].position = 990;
        let _ = r.update(Message::Play(false));
        assert_eq!(state(&r).position, 0.0);
        loaded(&mut r, &fake);
        fake.at(980.0, false, false);
        let _ = r.update(Message::Stop);
        assert_eq!(r.items[0].position, 0);

        // A player that fails stops with an error; so does a failed load.
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);
        *fake.state.lock().unwrap() = None;
        let _ = r.update(Message::PlayerTick);
        assert!(r.playback.is_none());
        assert!(r.error.as_deref().unwrap().starts_with("Playback stopped"));
        let _ = r.update(Message::Play(false));
        let generation = r.playback_generation;
        let _ = r.update(Message::PlayerLoaded(generation, Err("no codec".into())));
        assert!(r.playback.is_none());
        assert_eq!(r.error.as_deref(), Some("no codec"));
    }

    #[test]
    fn what_cannot_be_played() {
        let (mut r, _) = podcast_reader();
        r.start_remote = || None;
        let _ = r.update(Message::Play(false));
        assert!(r.playback.is_none(), "nothing open");
        let _ = r.update(Message::OpenItem(2));
        let _ = r.update(Message::Play(false));
        assert!(r.playback.is_none(), "a post");

        // Audio only: no video to watch, but its audio plays.
        let _ = r.update(Message::OpenItem(1));
        let media = r.article.as_mut().unwrap().media.as_mut().unwrap();
        media.enclosures.truncate(1);
        media.duration = None;
        let _ = r.update(Message::Play(true));
        assert!(r.playback.is_none());
        let _ = r.update(Message::Play(false));
        assert!(r.playback.is_some());
        assert!(r.remote.is_none(), "no media controls on this system");
        // A feed we don't know is named by its id.
        r.feeds.clear();
        r.start_remote = || Some(Box::new(FakeRemote));
        r.article.as_mut().unwrap().id = 3;
        r.items[0].id = 3;
        let _ = r.update(Message::Play(false));
        assert!(
            remote()
                .last()
                .unwrap()
                .starts_with("Item 1 / a / None / None")
        );
    }

    #[test]
    fn late_chapters_and_odd_cases() {
        let (mut r, fake) = podcast_reader();
        let _ = r.update(Message::OpenItem(1));
        let _ = r.update(Message::Play(false));
        loaded(&mut r, &fake);
        // Chapters that arrive after playing started reach the player too.
        let _ = r.update(Message::ChaptersLoaded(1, Ok(chapters())));
        assert_eq!(r.playback.as_ref().unwrap().chapters.len(), 4);
        // Without a known duration, seeking has no end.
        r.playback.as_mut().unwrap().state.duration = None;
        let _ = r.update(Message::SeekTo(5000.0));
        assert_eq!(state(&r).position, 5000.0);
        let _ = r.update(Message::TranscriptLoaded(1, Err("502".into())));
        assert_eq!(r.error.as_deref(), Some("502"));
    }

    #[tokio::test]
    async fn podcast_tasks_reach_the_server_and_the_player() {
        let (base, _requests) = crate::test_server::routed(vec![
            (
                "GET /api/v1/items/1/chapters",
                200,
                r#"{"chapters":[{"start":0.0}]}"#,
            ),
            ("GET /api/v1/items/1/transcript", 200, r#"{"segments":[]}"#),
            (
                "GET /api/v1/items/1",
                200,
                r#"{"id":1,"feed_id":"a","title":"Item 1","url":null,
                "author":null,"summary":null,"published":1,"read":false,"starred":false}"#,
            ),
            ("PUT /api/v1/items/1/", 204, ""),
        ]);
        let (mut r, _fake) = podcast_reader();
        r.client = Client::new(&base, Some("t".into())).unwrap();
        r.load_player = |_, _, _| Ok(crate::player::tests::Fake::default().boxed());
        let opened = run(r.update(Message::OpenItem(1)), 2).await;
        assert!(
            opened
                .iter()
                .any(|m| matches!(m, Message::ChaptersLoaded(1, Ok(_)))),
            "{opened:?}"
        );
        let shown = run(r.update(Message::ToggleTranscript), 1).await;
        assert!(matches!(
            shown[..],
            [Message::TranscriptLoaded(1, Ok(Some(_)))]
        ));
        let playing = run(r.update(Message::Play(false)), 1).await;
        assert!(matches!(playing[..], [Message::PlayerLoaded(_, Ok(_))]));
        for message in playing {
            let _ = r.update(message);
        }
        assert!(r.playback.as_ref().unwrap().player.is_some());

        // Playing, the app ticks; the tick saves through the server.
        let (mut app, dir) = setup_app("podcast-tasks");
        app.screen = Screen::Reader(Box::new(r));
        let _ = app.subscription();
        let saved = run(app.update(Message::Stop), 1).await;
        assert!(matches!(saved[..], [Message::PositionSaved(Ok(()))]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signing_out_stops_playback() {
        let (mut app, dir) = setup_app("signout");
        let (r, fake) = podcast_reader();
        app.screen = Screen::Reader(Box::new(r));
        let _ = app.update(Message::OpenItem(1));
        let _ = app.update(Message::Play(false));
        if let Screen::Reader(r) = &mut app.screen {
            loaded(r, &fake);
        }
        let _ = app.update(Message::SignOut);
        assert!(fake.calls().contains(&"stop".to_owned()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn media_keys_and_shortcuts() {
        use keyboard::{Key, Modifiers, key::Named};
        let press = |key: Key| keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key.clone(),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        };
        let cases = [
            ("p", Message::Play(false)),
            ("V", Message::Play(true)),
            ("x", Message::Stop),
            (",", Message::SeekBy(-controls::BACK)),
            (".", Message::SeekBy(controls::FORWARD)),
            ("[", Message::Chapter(false)),
            ("]", Message::Chapter(true)),
            ("t", Message::ToggleTranscript),
        ];
        for (key, expected) in cases {
            let got = on_key(press(Key::Character(key.into())));
            assert_eq!(format!("{got:?}"), format!("{:?}", Some(expected)), "{key}");
        }
        let space = on_key(press(Key::Named(Named::Space)));
        assert!(matches!(space, Some(Message::Play(false))));
    }
}
