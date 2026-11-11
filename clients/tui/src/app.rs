// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Client state and key handling, kept apart from drawing so it can be
//! tested without a terminal.

use anyhow::Result;
use pressbox_api::{Chapter, Chapters, Feed, Item, ItemPage, ItemQuery, MarkRead, Transcript};
use ratatui::crossterm::event::KeyCode;

use crate::{
    client::Client,
    player::{Mpv, Player, PlayerState},
};

const PAGE: u32 = 100;
/// While playing, the position is saved after it moves this many seconds.
const SAVE_EVERY: u32 = 15;
/// Stopping this close to the end (seconds) counts as finishing.
const NEAR_END: f64 = 30.0;
/// `,` and `.` seek back and forward this far (seconds).
const SEEK_BACK: f64 = 10.0;
const SEEK_FORWARD: f64 = 30.0;

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
    fn set_position(&self, id: i64, position: u32) -> Result<()>;
    fn chapters(&self, id: i64) -> Result<Chapters>;
    fn transcript(&self, id: i64) -> Result<Option<Transcript>>;
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
    fn set_position(&self, id: i64, position: u32) -> Result<()> {
        Client::set_position(self, id, position)
    }
    fn chapters(&self, id: i64) -> Result<Chapters> {
        Client::chapters(self, id)
    }
    fn transcript(&self, id: i64) -> Result<Option<Transcript>> {
        Client::transcript(self, id)
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
    /// The episode's chapters, if it has any.
    pub chapters: Vec<Chapter>,
    /// Loaded the first time `t` shows it; `Some(None)`: there is none.
    pub transcript: Option<Option<Transcript>>,
    /// Showing the transcript instead of the text.
    pub show_transcript: bool,
}

impl Reader {
    pub fn new(item: Item) -> Self {
        Self {
            item,
            scroll: 0,
            chapters: Vec::new(),
            transcript: None,
            show_transcript: false,
        }
    }
}

/// The episode being played.
pub struct Playing {
    pub item: Item,
    pub chapters: Vec<Chapter>,
    /// As of the last tick.
    pub state: PlayerState,
    pub video: bool,
    /// The position last saved on the server.
    saved: u32,
}

impl Playing {
    pub fn new(item: Item, chapters: Vec<Chapter>, state: PlayerState, video: bool) -> Self {
        let mut playing = Self {
            item,
            chapters,
            state,
            video,
            saved: 0,
        };
        playing.saved = playing.position();
        playing
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
    pub player: Box<dyn Player>,
    pub playing: Option<Playing>,
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
            player: Box::new(Mpv::new("mpv")),
            playing: None,
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
                KeyCode::Char('p') => self.play_or_pause(backend, false)?,
                KeyCode::Char('V') => self.play_or_pause(backend, true)?,
                KeyCode::Char('x') => self.stop(backend)?,
                KeyCode::Char(',') => self.seek(-SEEK_BACK)?,
                KeyCode::Char('.') => self.seek(SEEK_FORWARD)?,
                KeyCode::Char(']') => self.chapter(true)?,
                KeyCode::Char('[') => self.chapter(false)?,
                KeyCode::Char('+') => self.speed(0.25)?,
                KeyCode::Char('-') => self.speed(-0.25)?,
                KeyCode::Char('t') => self.toggle_transcript(backend)?,
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
        let mut reader = Reader::new(item);
        if reader
            .item
            .media
            .as_ref()
            .is_some_and(|m| m.chapters.is_some())
        {
            // Not worth failing the article for.
            reader.chapters = backend.chapters(id).map(|c| c.chapters).unwrap_or_default();
        }
        // An episode isn't read until it is played to the end.
        let episode = reader.item.media.is_some();
        self.reader = Some(reader);
        self.focus = Pane::Reader;
        if episode {
            Ok(())
        } else {
            self.mark(backend, true)
        }
    }

    /// `p`/`V`: pauses or resumes the episode playing, or starts the
    /// current item's (its video, with `video`).
    fn play_or_pause(&mut self, backend: &impl Backend, video: bool) -> Result<()> {
        let current = self.current_item().filter(|i| i.media.is_some()).cloned();
        let same = |p: &Playing| current.as_ref().is_none_or(|c| c.id == p.item.id);
        if let Some(playing) = self
            .playing
            .as_mut()
            .filter(|p| same(p) && p.video == video)
        {
            self.player.toggle_pause()?;
            playing.state.paused = !playing.state.paused;
            if playing.state.paused {
                // Pausing is a good moment to save; the next tick does.
                playing.saved = u32::MAX;
            }
            return Ok(());
        }
        let Some(item) = current else {
            self.status = "nothing to play here".into();
            return Ok(());
        };
        let media = item.media.clone().unwrap_or_default();
        let enclosure = if video {
            media.video()
        } else {
            media.audio().or_else(|| media.video())
        };
        let Some(enclosure) = enclosure else {
            self.status = "this episode has no video".into();
            return Ok(());
        };
        self.stop(backend)?;
        // Stopping may have just saved a newer position (switching between
        // an episode's audio and video).
        let position = self
            .items
            .iter()
            .find(|i| i.id == item.id)
            .map_or(item.position, |i| i.position);
        // Played to the end before: start again.
        let near_end = |d: u32| f64::from(position) >= f64::from(d) - NEAR_END;
        let start = if media.duration.is_some_and(near_end) {
            0
        } else {
            position
        };
        self.player
            .load(&enclosure.url, &item.title, start, video)?;
        let chapters = match self.reader.as_ref().filter(|r| r.item.id == item.id) {
            Some(r) => r.chapters.clone(),
            None if media.chapters.is_some() => backend
                .chapters(item.id)
                .map(|c| c.chapters)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        self.status = format!("playing {}", item.title);
        let state = PlayerState {
            position: f64::from(start),
            duration: media.duration.map(f64::from),
            paused: false,
            speed: 1.0,
            ended: false,
        };
        self.playing = Some(Playing::new(item, chapters, state, video));
        Ok(())
    }

    /// Called about once a second while playing: follows the player, saves
    /// the position now and then, and notices the end.
    pub fn tick(&mut self, backend: &impl Backend) {
        let Some(playing) = self.playing.as_mut() else {
            return;
        };
        let result = match self.player.state() {
            Some(state) if !state.ended => {
                playing.state = state;
                let position = playing.position();
                let moved = position.abs_diff(playing.saved) >= SAVE_EVERY
                    || (state.paused && position != playing.saved);
                if moved {
                    playing.saved = position;
                    let id = playing.item.id;
                    self.update_item(id, |i| i.position = position);
                    backend.set_position(id, position)
                } else {
                    Ok(())
                }
            }
            Some(_) => self.finish(backend, true),
            // The user closed mpv's window.
            None => self.finish(backend, false),
        };
        self.report(result);
    }

    /// `x`: stops playing, saving the position.
    pub fn stop(&mut self, backend: &impl Backend) -> Result<()> {
        if self.playing.is_none() {
            return Ok(());
        }
        if let Some(state) = self.player.state()
            && let Some(playing) = self.playing.as_mut()
        {
            playing.state = state;
        }
        let ended = self.playing.as_ref().is_some_and(|p| p.state.ended);
        self.finish(backend, ended)
    }

    /// Stops playing. Played to (near) the end, the episode is marked read
    /// and its position forgotten; otherwise the position is saved.
    fn finish(&mut self, backend: &impl Backend, ended: bool) -> Result<()> {
        let Some(playing) = self.playing.take() else {
            return Ok(());
        };
        self.player.stop();
        let near_end = playing
            .state
            .duration
            .is_some_and(|d| playing.state.position >= d - NEAR_END);
        let item = &playing.item;
        if ended || near_end {
            backend.set_position(item.id, 0)?;
            self.update_item(item.id, |i| i.position = 0);
            let read = self
                .items
                .iter()
                .find(|i| i.id == item.id)
                .map_or(item.read, |i| i.read);
            if !read {
                self.set_read(backend, item.id, &item.feed_id, true)?;
            }
            self.status = format!("finished {}", item.title);
        } else {
            let position = playing.position();
            backend.set_position(item.id, position)?;
            self.update_item(item.id, |i| i.position = position);
            self.status = format!("stopped {}", item.title);
        }
        Ok(())
    }

    fn playing_mut(&mut self) -> Result<&mut Playing> {
        self.playing
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("nothing is playing"))
    }

    /// `,` and `.`: seeks back or forward.
    fn seek(&mut self, by: f64) -> Result<()> {
        self.playing_mut()?;
        self.player.seek(by, false)?;
        let playing = self.playing_mut()?;
        playing.state.position = (playing.state.position + by).max(0.0);
        Ok(())
    }

    /// `]` and `[`: the next chapter, or back to the start of this one (the
    /// one before, near its start).
    fn chapter(&mut self, next: bool) -> Result<()> {
        let playing = self.playing_mut()?;
        let position = playing.state.position;
        let mut starts = playing
            .chapters
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| c.start);
        let to = if next {
            starts.find(|s| *s > position + 0.5)
        } else {
            Some(starts.rfind(|s| *s < position - 3.0).unwrap_or(0.0))
        };
        let Some(to) = to else {
            self.status = "no more chapters".into();
            return Ok(());
        };
        self.player.seek(to, true)?;
        self.playing_mut()?.state.position = to;
        Ok(())
    }

    /// `+` and `-`: plays faster or slower, from half to three times speed.
    fn speed(&mut self, by: f64) -> Result<()> {
        let playing = self.playing_mut()?;
        let speed = (playing.state.speed + by).clamp(0.5, 3.0);
        self.player.set_speed(speed)?;
        self.playing_mut()?.state.speed = speed;
        self.status = format!("speed {speed}x");
        Ok(())
    }

    /// `t` in the reader: shows the episode's transcript, or the text again.
    fn toggle_transcript(&mut self, backend: &impl Backend) -> Result<()> {
        let Some(reader) = self.reader.as_mut().filter(|_| self.focus == Pane::Reader) else {
            return Ok(());
        };
        let has = reader
            .item
            .media
            .as_ref()
            .is_some_and(|m| !m.transcripts.is_empty());
        if !has {
            self.status = "no transcript".into();
            return Ok(());
        }
        if reader.transcript.is_none() {
            reader.transcript = Some(backend.transcript(reader.item.id)?);
        }
        reader.show_transcript = !reader.show_transcript;
        reader.scroll = 0;
        Ok(())
    }

    /// Sets the read flag of the current item, locally and on the server,
    /// unless it is already set that way.
    fn mark(&mut self, backend: &impl Backend, read: bool) -> Result<()> {
        let Some(item) = self.current_item().filter(|i| i.read != read) else {
            return Ok(());
        };
        let (id, feed_id) = (item.id, item.feed_id.clone());
        self.set_read(backend, id, &feed_id, read)
    }

    /// Sets an item's read flag on the server and here.
    fn set_read(
        &mut self,
        backend: &impl Backend,
        id: i64,
        feed_id: &str,
        read: bool,
    ) -> Result<()> {
        backend.set_read(id, read)?;
        self.update_item(id, |i| i.read = read);
        if let Some(p) = self.playing.as_mut().filter(|p| p.item.id == id) {
            p.item.read = read;
        }
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
            media: None,
            position: 0,
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
                    image: None,
                    medium: None,
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
        fn set_position(&self, id: i64, position: u32) -> Result<()> {
            if self.offline.get() {
                anyhow::bail!("offline");
            }
            self.calls
                .borrow_mut()
                .push(format!("position {id} {position}"));
            Ok(())
        }
        fn chapters(&self, id: i64) -> Result<Chapters> {
            self.calls.borrow_mut().push(format!("chapters {id}"));
            let chapter = |start: f64, title: &str, hidden| Chapter {
                start,
                title: Some(title.into()),
                hidden,
                ..Chapter::default()
            };
            Ok(Chapters {
                chapters: vec![
                    chapter(0.0, "Intro", false),
                    chapter(60.0, "art", true),
                    chapter(120.0, "Main", false),
                    chapter(600.0, "End", false),
                ],
            })
        }
        fn transcript(&self, id: i64) -> Result<Option<Transcript>> {
            self.calls.borrow_mut().push(format!("transcript {id}"));
            Ok(Some(Transcript {
                segments: vec![pressbox_api::Segment {
                    start: 0.0,
                    end: 5.0,
                    speaker: None,
                    text: "Hello".into(),
                }],
            }))
        }
    }

    /// A player that records what it is told, with a state tests set.
    #[derive(Clone, Default)]
    struct FakePlayer {
        calls: std::rc::Rc<RefCell<Vec<String>>>,
        state: std::rc::Rc<RefCell<Option<PlayerState>>>,
        broken: std::rc::Rc<std::cell::Cell<bool>>,
    }

    impl FakePlayer {
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
        fn at(&self, position: f64, paused: bool, ended: bool) {
            *self.state.borrow_mut() = Some(PlayerState {
                position,
                duration: Some(1000.0),
                paused,
                speed: 1.0,
                ended,
            });
        }
    }

    impl Player for FakePlayer {
        fn load(&mut self, url: &str, title: &str, start: u32, video: bool) -> Result<()> {
            if self.broken.get() {
                anyhow::bail!("no mpv");
            }
            self.calls
                .borrow_mut()
                .push(format!("load {url} {title:?} {start} {video}"));
            self.at(f64::from(start), false, false);
            Ok(())
        }
        fn state(&mut self) -> Option<PlayerState> {
            *self.state.borrow()
        }
        fn toggle_pause(&mut self) -> Result<()> {
            self.calls.borrow_mut().push("pause".into());
            Ok(())
        }
        fn seek(&mut self, seconds: f64, absolute: bool) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("seek {seconds} {absolute}"));
            if let Some(state) = self.state.borrow_mut().as_mut() {
                state.position = if absolute {
                    seconds
                } else {
                    (state.position + seconds).max(0.0)
                };
            }
            Ok(())
        }
        fn set_speed(&mut self, speed: f64) -> Result<()> {
            self.calls.borrow_mut().push(format!("speed {speed}"));
            Ok(())
        }
        fn stop(&mut self) {
            self.calls.borrow_mut().push("stop".into());
            *self.state.borrow_mut() = None;
        }
    }

    /// Item 1 is an episode with audio, video, chapters and a transcript,
    /// started at 100s; item 4 has only audio.
    fn podcast() -> (Fake, App, FakePlayer) {
        use pressbox_api::{Enclosure, Media, TranscriptFile};
        let fake = Fake::new();
        let enclosure = |url: &str, mime: &str| Enclosure {
            url: url.into(),
            mime: Some(mime.into()),
            ..Enclosure::default()
        };
        {
            let mut items = fake.items.borrow_mut();
            items[0].media = Some(Media {
                enclosures: vec![
                    enclosure("https://x/1.mp3", "audio/mpeg"),
                    enclosure("https://x/1.mp4", "video/mp4"),
                ],
                duration: Some(1000),
                chapters: Some("https://x/1.json".into()),
                transcripts: vec![TranscriptFile {
                    url: "https://x/1.vtt".into(),
                    mime: "text/vtt".into(),
                    ..TranscriptFile::default()
                }],
                ..Media::default()
            });
            items[0].position = 100;
            let mut audio = item(4, "b");
            audio.media = Some(Media {
                enclosures: vec![enclosure("https://x/4.mp3", "audio/mpeg")],
                ..Media::default()
            });
            items.push(audio);
        }
        let player = FakePlayer::default();
        let mut app = App::new();
        app.player = Box::new(player.clone());
        app.reload(&fake);
        app.focus = Pane::Items;
        (fake, app, player)
    }

    #[test]
    fn plays_resumes_and_saves_the_position() {
        let (fake, mut app, player) = podcast();
        keys(&mut app, &fake, "p");
        assert_eq!(
            player.calls(),
            ["load https://x/1.mp3 \"item 1\" 100 false"]
        );
        assert_eq!(app.status, "playing item 1");
        let playing = app.playing.as_ref().unwrap();
        assert_eq!(playing.chapters.len(), 4, "chapters fetched for the list");
        assert_eq!(playing.chapter().unwrap().title.as_deref(), Some("Intro"));

        // A tick follows the player; the position is saved every 15s.
        player.at(110.0, false, false);
        app.tick(&fake);
        assert_eq!(app.playing.as_ref().unwrap().state.position, 110.0);
        player.at(130.0, false, false);
        app.tick(&fake);
        assert_eq!(app.items[0].position, 130);
        assert_eq!(
            app.playing
                .as_ref()
                .unwrap()
                .chapter()
                .unwrap()
                .title
                .as_deref(),
            Some("Main")
        );

        // Pausing saves at once (on the next tick).
        keys(&mut app, &fake, "p");
        assert!(app.playing.as_ref().unwrap().state.paused);
        player.at(131.0, true, false);
        app.tick(&fake);
        app.tick(&fake);
        keys(&mut app, &fake, "p");
        assert!(!app.playing.as_ref().unwrap().state.paused);

        // Stopping saves where it was, and the episode stays unread.
        keys(&mut app, &fake, "x");
        assert_eq!(app.status, "stopped item 1");
        assert!(app.playing.is_none());
        assert!(!app.items[0].read);
        let calls = fake.calls.borrow().clone();
        assert_eq!(
            calls,
            [
                "chapters 1",
                "position 1 130",
                "position 1 131",
                "position 1 131",
            ]
        );
        assert_eq!(player.calls()[1..], ["pause", "pause", "stop"]);
        keys(&mut app, &fake, "x");
        assert_eq!(app.status, "", "nothing to stop");
    }

    #[test]
    fn finishing_marks_read_and_forgets_the_position() {
        let (fake, mut app, player) = podcast();
        keys(&mut app, &fake, "p");
        player.at(1000.0, true, true);
        app.tick(&fake);
        assert_eq!(app.status, "finished item 1");
        assert!(app.playing.is_none());
        assert!(app.items[0].read);
        assert_eq!(app.items[0].position, 0);
        assert_eq!(fake.calls.borrow()[1..], ["position 1 0", "read 1 true"]);

        // Played to the end before: it starts again from the top.
        app.items[0].position = 990;
        keys(&mut app, &fake, "p");
        assert!(player.calls().last().unwrap().ends_with(" 0 false"));
        // Stopped near the end counts as finished; already read stays so.
        player.at(980.0, false, false);
        keys(&mut app, &fake, "x");
        assert_eq!(app.status, "finished item 1");
        assert_eq!(fake.calls.borrow().last().unwrap(), "position 1 0");

        // Closing mpv's window part way through saves the position.
        keys(&mut app, &fake, "p");
        player.at(200.0, false, false);
        app.tick(&fake);
        *player.state.borrow_mut() = None;
        app.tick(&fake);
        assert_eq!(app.status, "stopped item 1");
        assert_eq!(fake.calls.borrow().last().unwrap(), "position 1 200");
        app.tick(&fake);
    }

    #[test]
    fn seeking_chapters_speed_and_video() {
        let (fake, mut app, player) = podcast();
        for key in ",.[]+-".chars() {
            keys(&mut app, &fake, &key.to_string());
            assert_eq!(app.status, "error: nothing is playing", "{key}");
        }
        keys(&mut app, &fake, "p");
        keys(&mut app, &fake, ".");
        assert_eq!(app.playing.as_ref().unwrap().state.position, 130.0);
        keys(&mut app, &fake, ",,,,,,,,,,,,,,,");
        assert_eq!(
            app.playing.as_ref().unwrap().state.position,
            0.0,
            "not before 0"
        );
        keys(&mut app, &fake, "]");
        assert_eq!(
            app.playing.as_ref().unwrap().state.position,
            120.0,
            "hidden ones skipped"
        );
        keys(&mut app, &fake, "]]");
        assert_eq!(app.status, "no more chapters");
        keys(&mut app, &fake, "[");
        assert_eq!(
            app.playing.as_ref().unwrap().state.position,
            120.0,
            "start of this one"
        );
        app.playing.as_mut().unwrap().state.position = 121.0;
        keys(&mut app, &fake, "[");
        assert_eq!(
            app.playing.as_ref().unwrap().state.position,
            0.0,
            "near its start: back one"
        );
        keys(&mut app, &fake, "++++++++++");
        assert_eq!(app.status, "speed 3x");
        keys(&mut app, &fake, "-");
        assert_eq!(app.playing.as_ref().unwrap().state.speed, 2.75);
        assert!(player.calls().contains(&"seek 600 true".to_owned()));
        assert!(player.calls().contains(&"speed 2.75".to_owned()));

        // V switches to the video.
        keys(&mut app, &fake, "V");
        assert_eq!(
            player.calls().last().unwrap(),
            "load https://x/1.mp4 \"item 1\" 0 true"
        );
        assert!(app.playing.as_ref().unwrap().video);
        keys(&mut app, &fake, "V");
        assert_eq!(player.calls().last().unwrap(), "pause");

        // An audio-only episode has no video; items without media can't play.
        keys(&mut app, &fake, "jjj");
        keys(&mut app, &fake, "V");
        assert_eq!(app.status, "this episode has no video");
        keys(&mut app, &fake, "k");
        keys(&mut app, &fake, "p");
        assert_eq!(app.status, "nothing to play here");
        keys(&mut app, &fake, "jp");
        assert_eq!(app.status, "playing item 4");
        assert!(app.playing.as_ref().unwrap().chapters.is_empty());
    }

    #[test]
    fn the_reader_shows_chapters_and_transcripts() {
        let (fake, mut app, player) = podcast();
        keys(&mut app, &fake, "\n");
        assert_eq!(app.focus, Pane::Reader);
        let reader = app.reader.as_ref().unwrap();
        assert_eq!(reader.chapters.len(), 4);
        assert!(!reader.item.read, "episodes are read once played through");

        keys(&mut app, &fake, "t");
        let reader = app.reader.as_ref().unwrap();
        assert!(reader.show_transcript);
        assert_eq!(
            reader.transcript.clone().unwrap().unwrap().segments[0].text,
            "Hello"
        );
        keys(&mut app, &fake, "tt");
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .filter(|c| c.starts_with("transcript"))
                .count(),
            1,
            "fetched once"
        );

        // Playing from the reader uses the chapters it has.
        keys(&mut app, &fake, "p");
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .filter(|c| c.starts_with("chapters"))
                .count(),
            1
        );
        assert_eq!(player.calls().len(), 1);

        // Items without a transcript, and `t` outside the reader.
        keys(&mut app, &fake, "h");
        keys(&mut app, &fake, "t");
        keys(&mut app, &fake, "jjj\n");
        keys(&mut app, &fake, "t");
        assert_eq!(app.status, "no transcript");

        // A player that can't start says so.
        player.broken.set(true);
        keys(&mut app, &fake, "p");
        assert_eq!(app.status, "error: no mpv");
    }

    #[test]
    fn saving_failures_are_reported() {
        let (fake, mut app, player) = podcast();
        keys(&mut app, &fake, "p");
        fake.offline.set(true);
        player.at(500.0, false, false);
        app.tick(&fake);
        assert_eq!(app.status, "error: offline");
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
