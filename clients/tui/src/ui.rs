// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Drawing.

use chrono::{Local, TimeZone};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{ALL_ROW, App, FIXED_ROWS, Pane, STARRED_ROW};

const HELP: &str = "j/k move  enter open  / search  m read  M read below  s star  o browser  A all read  u unread/all/read  r reload  R refresh  q quit";

pub fn draw(frame: &mut Frame, app: &App) {
    let [body, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [feeds, right] =
        Layout::horizontal([Constraint::Percentage(25), Constraint::Min(20)]).areas(body);

    draw_feeds(frame, app, feeds);
    if app.reader.is_some() {
        let [items, reader] =
            Layout::vertical([Constraint::Percentage(35), Constraint::Min(5)]).areas(right);
        draw_items(frame, app, items);
        draw_reader(frame, app, reader);
    } else {
        draw_items(frame, app, right);
    }

    let line = if let Some(text) = &app.editing {
        Line::from(vec![
            Span::raw("/").cyan().bold(),
            Span::raw(text.clone()),
            Span::raw("█").cyan(),
            Span::raw("   enter search  esc cancel").dark_gray(),
        ])
    } else if app.status.is_empty() {
        Line::from(HELP).dark_gray()
    } else if app.status.starts_with("error") {
        Line::from(app.status.as_str()).red()
    } else {
        Line::from(app.status.as_str()).yellow()
    };
    frame.render_widget(line, status);
}

fn block(title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Block::bordered().title(title).border_style(style)
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::new().add_modifier(Modifier::REVERSED)
    }
}

fn draw_feeds(frame: &mut Frame, app: &App, area: Rect) {
    let mut rows = Vec::with_capacity(app.feeds.len() + FIXED_ROWS);
    rows.insert(
        ALL_ROW,
        ListItem::new(format!("All items ({})", app.total_unread())).bold(),
    );
    rows.insert(STARRED_ROW, ListItem::new("★ Starred").bold());
    for feed in &app.feeds {
        let title = Span::raw(feed.title.clone());
        let mut spans = vec![if feed.archived {
            title.dark_gray()
        } else {
            title
        }];
        if feed.archived {
            spans.push(Span::raw(" (removed)").dark_gray().italic());
        }
        if feed.unread > 0 {
            spans.push(Span::raw(format!(" ({})", feed.unread)).cyan());
        }
        if feed.last_error.is_some() {
            spans.push(Span::raw(" !").red().bold());
        }
        rows.push(ListItem::new(Line::from(spans)));
    }
    let focused = app.focus == Pane::Feeds;
    let list = List::new(rows)
        .block(block(" Feeds ".into(), focused))
        .highlight_style(highlight(focused));
    let mut state = ListState::default().with_selected(Some(app.feed_row));
    frame.render_stateful_widget(list, area, &mut state);
}

fn date(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map_or_else(String::new, |d| d.format("%b %d").to_string())
}

fn draw_items(frame: &mut Frame, app: &App, area: Rect) {
    let show_feed = app.feed_row < FIXED_ROWS;
    let rows: Vec<ListItem> = app
        .items
        .iter()
        .map(|item| {
            let mut spans = vec![
                Span::raw(if item.read { "  " } else { "● " }).cyan(),
                Span::raw(if item.starred { "★ " } else { "  " }).yellow(),
                Span::raw(format!("{:<7}", date(item.published))).dark_gray(),
            ];
            if show_feed {
                let title: String = app.feed_title(&item.feed_id).chars().take(16).collect();
                spans.push(Span::raw(format!("{title:<17}")).magenta());
            }
            let title = Span::raw(item.title.clone());
            spans.push(if item.read { title } else { title.bold() });
            ListItem::new(Line::from(spans))
        })
        .collect();
    let focused = app.focus == Pane::Items;
    let more = if app.next_page.is_some() { "+" } else { "" };
    let title = if let Some(q) = &app.search {
        format!(" Search {q:?}: {}{more} (esc clears) ", app.items.len())
    } else {
        let what = if app.filters_by_read() {
            app.show.label()
        } else {
            "all"
        };
        format!(" Items: {} {what}{more} ", app.items.len())
    };
    let list = List::new(rows)
        .block(block(title, focused))
        .highlight_style(highlight(focused));
    let mut state =
        ListState::default().with_selected((!app.items.is_empty()).then_some(app.item_row));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_reader(frame: &mut Frame, app: &App, area: Rect) {
    let Some(reader) = &app.reader else { return };
    let item = &reader.item;
    let width = usize::from(area.width.saturating_sub(2)).max(20);
    let mut meta = vec![app.feed_title(&item.feed_id).to_owned()];
    if let Some(author) = &item.author {
        meta.push(author.clone());
    }
    meta.push(date(item.published));
    let mut lines = vec![
        Line::from(item.title.clone()).bold(),
        Line::from(meta.join(" · ")).dark_gray(),
    ];
    if let Some(url) = &item.url {
        lines.push(Line::from(url.clone()).blue().underlined());
    }
    lines.push(Line::default());
    let html = item
        .content
        .as_deref()
        .or(item.summary.as_deref())
        .unwrap_or("");
    let body = html2text::from_read(html.as_bytes(), width).unwrap_or_else(|_| html.to_owned());
    lines.extend(body.lines().map(|l| Line::from(l.to_owned())));

    let flags = match (item.read, item.starred) {
        (_, true) => " ★ ",
        (false, false) => " unread ",
        (true, false) => "",
    };
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block(format!(" Reader{flags}"), app.focus == Pane::Reader))
        .wrap(Wrap { trim: false })
        .scroll((reader.scroll, 0));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use pressbox_api::{Feed, Item};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::app::{Reader, Show};

    fn feed(id: &str, unread: u32) -> Feed {
        Feed {
            id: id.into(),
            title: format!("Feed {id}"),
            url: format!("https://{id}.example/"),
            site_url: None,
            tags: Vec::new(),
            unread,
            total: unread,
            last_fetched: None,
            last_error: None,
            archived: false,
        }
    }

    fn item(id: i64, feed: &str, read: bool, starred: bool) -> Item {
        Item {
            id,
            feed_id: feed.into(),
            title: format!("Story {id}"),
            url: Some(format!("https://{feed}.example/{id}")),
            author: Some("Ana".into()),
            summary: Some("<p>The summary.</p>".into()),
            content: None,
            published: 1_793_700_000,
            read,
            starred,
        }
    }

    /// An app with two feeds (one failing, one removed) and two items.
    fn app() -> App {
        let mut app = App::new();
        let mut failing = feed("a", 2);
        failing.last_error = Some("HTTP 500".into());
        let mut removed = feed("old", 0);
        removed.archived = true;
        app.feeds = vec![failing, feed("b", 0), removed];
        app.items = vec![item(1, "a", false, true), item(2, "b", true, false)];
        app
    }

    /// The screen as text, one string per row.
    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    fn contains(rows: &[String], text: &str) -> bool {
        rows.iter().any(|r| r.contains(text))
    }

    #[test]
    fn draws_feeds_items_and_help() {
        let rows = screen(&app(), 120, 12);
        for text in [
            " Feeds ",
            "All items (2)",
            "★ Starred",
            "Feed a (2) !",
            "Feed old (removed)",
            " Items: 2 unread ",
            "● ★ ",
            "Feed a",
            "Story 1",
            "Story 2",
            "j/k move",
        ] {
            assert!(
                contains(&rows, text),
                "{text:?} missing:\n{}",
                rows.join("\n")
            );
        }
        assert!(
            !contains(&rows, "Feed b ("),
            "no count when nothing is unread"
        );
    }

    #[test]
    fn titles_follow_the_view() {
        let mut app = app();
        app.next_page = Some(crate::app::NextPage::Cursor("c".into()));
        app.show = Show::Read;
        assert!(contains(&screen(&app, 120, 8), " Items: 2 read+ "));
        app.feed_row = STARRED_ROW;
        assert!(contains(&screen(&app, 120, 8), " Items: 2 all+ "));
        app.search = Some("rust".into());
        assert!(contains(
            &screen(&app, 120, 8),
            " Search \"rust\": 2+ (esc clears) "
        ));

        // One feed: no feed column.
        app.search = None;
        app.feed_row = FIXED_ROWS;
        let rows = screen(&app, 120, 8);
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("Feed a") && r.contains("Story 1"))
        );
    }

    #[test]
    fn draws_the_reader() {
        let mut app = app();
        app.reader = Some(Reader {
            item: item(1, "a", false, false),
            scroll: 0,
        });
        app.focus = Pane::Reader;
        let rows = screen(&app, 100, 24);
        for text in [
            " Reader unread ",
            "Story 1",
            "Feed a · Ana · ",
            "https://a.example/1",
            "The summary.",
        ] {
            assert!(
                contains(&rows, text),
                "{text:?} missing:\n{}",
                rows.join("\n")
            );
        }
        app.reader.as_mut().unwrap().item.starred = true;
        assert!(contains(&screen(&app, 100, 24), " Reader ★ "));
        let reader = app.reader.as_mut().unwrap();
        (reader.item.starred, reader.item.read) = (false, true);
        let rows = screen(&app, 100, 24);
        let title = rows.iter().find(|r| r.contains(" Reader")).unwrap();
        assert!(
            !title.contains("unread") && !title.contains('★'),
            "nothing to flag: {title}"
        );
    }

    #[test]
    fn status_line_shows_prompts_and_messages() {
        let mut app = app();
        app.editing = Some("nix".into());
        let rows = screen(&app, 100, 6);
        assert!(
            rows[5].starts_with("/nix█   enter search  esc cancel"),
            "{}",
            rows[5]
        );

        app.editing = None;
        app.status = "error: offline".into();
        let mut terminal = Terminal::new(TestBackend::new(100, 6)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cell = &terminal.backend().buffer()[(0, 5)];
        assert_eq!((cell.symbol(), cell.fg), ("e", Color::Red));

        app.status = "reloaded".into();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cell = &terminal.backend().buffer()[(0, 5)];
        assert_eq!((cell.symbol(), cell.fg), ("r", Color::Yellow));
    }

    #[test]
    fn dates_are_short() {
        assert_eq!(date(i64::MAX), "");
        assert_eq!(date(1_793_700_000).len(), 6, "like \"Nov 03\"");
    }
}
