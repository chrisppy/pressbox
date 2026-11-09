// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Layout: the pairing screen, and the reader's three panes (sources,
//! items, article).

use chrono::{Local, TimeZone};
use iced::{
    Alignment, Element, Fill, Font, Length, Theme,
    widget::{button, column, container, row, rule, scrollable, space, text, text_input},
};
use pressbox_api::Item;

use crate::{
    app::{App, Flag, Message, Reader, Screen, Setup, Source},
    theme::Colors,
};

const SIDEBAR: f32 = 230.0;
const LIST: f32 = 360.0;

pub fn view(app: &App) -> Element<'_, Message> {
    match &app.screen {
        Screen::Setup(setup) => setup_view(setup),
        Screen::Reader(reader) => reader_view(reader),
    }
}

fn setup_view(setup: &Setup) -> Element<'_, Message> {
    let mut form = column![
        text("pressbox").size(32),
        text("Pair this computer with your pressbox server.").style(muted),
        space().height(8),
        text("Server").size(14),
        text_input("https://rss.example.org", &setup.server)
            .on_input_maybe((!setup.busy).then_some(Message::ServerChanged))
            .on_submit(Message::Pair)
            .padding(8),
        text("Device name").size(14),
        text_input("laptop", &setup.device)
            .on_input_maybe((!setup.busy).then_some(Message::DeviceChanged))
            .on_submit(Message::Pair)
            .padding(8),
        button(text("Pair this device"))
            .on_press_maybe((!setup.busy).then_some(Message::Pair))
            .padding([8, 16]),
    ]
    .spacing(8)
    .max_width(420);

    if let Some(code) = &setup.code {
        form = form.push(space().height(8)).push(
            column![
                text("Approve this code:").style(muted),
                text(code).size(36).font(Font::MONOSPACE),
                text(format!(
                    "On the server: sudo pressboxctl approve {code}\n\
                     Or from a paired device: pressbox approve {code}"
                ))
                .size(13)
                .font(Font::MONOSPACE)
                .style(muted),
                text("Waiting for approval…").size(13).style(muted),
            ]
            .spacing(6),
        );
    }
    if let Some(error) = &setup.error {
        form = form.push(text(error).style(error_text));
    }

    container(form).center(Fill).padding(24).into()
}

fn reader_view(reader: &Reader) -> Element<'_, Message> {
    let body = row![
        sidebar(reader),
        rule::vertical(1),
        item_list(reader),
        rule::vertical(1),
        article(reader),
    ]
    .height(Fill);

    let mut page = column![body];
    if let Some(banner) = banner(reader) {
        page = page.push(rule::horizontal(1)).push(banner);
    }
    container(page).style(background).into()
}

fn banner(reader: &Reader) -> Option<Element<'_, Message>> {
    let (message, style): (&str, fn(&Theme) -> text::Style) = match (&reader.error, &reader.status)
    {
        (Some(e), _) => (e, error_text),
        (None, Some(s)) => (s, muted),
        (None, None) => return None,
    };
    Some(
        row![
            text(message).style(style).width(Fill),
            button(text("Dismiss").size(13))
                .style(button::text)
                .on_press(Message::Dismiss),
        ]
        .align_y(Alignment::Center)
        .padding([4, 12])
        .into(),
    )
}

fn sidebar(reader: &Reader) -> Element<'_, Message> {
    let total_unread: u32 = reader.feeds.iter().map(|f| f.unread).sum();
    let mut list = column![
        source_button("Unread", Some(total_unread), Source::Unread, reader),
        source_button("All items", None, Source::All, reader),
        source_button("Starred", None, Source::Starred, reader),
    ]
    .spacing(2);

    let tags = reader.tags();
    if !tags.is_empty() {
        list = list.push(heading("Tags"));
        for (tag, unread) in tags {
            list = list.push(source_button(
                format!("#{tag}"),
                Some(unread),
                Source::Tag(tag),
                reader,
            ));
        }
    }

    list = list.push(heading("Feeds"));
    for feed in reader.feeds.iter().filter(|f| !f.archived) {
        let label = if feed.last_error.is_some() {
            format!("{} ⚠", feed.title)
        } else {
            feed.title.clone()
        };
        list = list.push(source_button(
            label,
            Some(feed.unread),
            Source::Feed(feed.id.clone()),
            reader,
        ));
    }
    let archived: Vec<_> = reader.feeds.iter().filter(|f| f.archived).collect();
    if !archived.is_empty() {
        list = list.push(heading("Archived"));
        for feed in archived {
            list = list.push(source_button(
                feed.title.clone(),
                None,
                Source::Feed(feed.id.clone()),
                reader,
            ));
        }
    }

    let footer = column![
        row![
            text_input("Approve code", &reader.approve_code)
                .on_input(Message::ApproveCodeChanged)
                .on_submit(Message::Approve)
                .size(13)
                .padding(6),
            button(text("Approve").size(13))
                .on_press(Message::Approve)
                .padding([6, 10]),
        ]
        .spacing(6),
        text(identity(reader)).size(12).style(muted),
        row![
            button(text("Refresh").size(13))
                .style(button::text)
                .on_press(Message::Refresh),
            button(text("Sign out").size(13))
                .style(button::text)
                .on_press(Message::SignOut),
        ],
    ]
    .spacing(6);

    column![
        scrollable(list.padding([8, 8])).height(Fill),
        container(footer).padding(8),
    ]
    .width(SIDEBAR)
    .into()
}

fn identity(reader: &Reader) -> String {
    let mut parts = vec![reader.client.server().to_owned()];
    if let Some(me) = &reader.me {
        parts.push(match &me.device {
            Some(d) => format!("{} on {d}", me.user),
            None => me.user.clone(),
        });
    }
    if let Some(v) = &reader.version {
        parts.push(format!("pressboxd {}", v.version));
    }
    parts.join("\n")
}

fn heading(label: &str) -> Element<'_, Message> {
    container(text(label).size(12).style(muted))
        .padding(iced::Padding {
            top: 14.0,
            bottom: 4.0,
            left: 8.0,
            right: 8.0,
        })
        .into()
}

fn source_button<'a>(
    label: impl text::IntoFragment<'a>,
    count: Option<u32>,
    source: Source,
    reader: &Reader,
) -> Element<'a, Message> {
    let selected = reader.source == source;
    let mut content = row![
        text(label)
            .size(14)
            .width(Fill)
            .wrapping(text::Wrapping::None)
    ]
    .align_y(Alignment::Center);
    if let Some(n) = count.filter(|n| *n > 0) {
        content = content.push(text(n.to_string()).size(12).style(muted));
    }
    button(content)
        .width(Fill)
        .padding([5, 8])
        .style(move |theme, status| row_style(theme, status, selected))
        .on_press(Message::Select(source))
        .into()
}

fn item_list(reader: &Reader) -> Element<'_, Message> {
    let header = column![
        row![
            text(reader.source_title()).size(18).width(Fill),
            button(text("Mark all read").size(13))
                .style(button::text)
                .on_press_maybe((reader.source != Source::Starred).then_some(Message::MarkAllRead)),
        ]
        .align_y(Alignment::Center),
        text_input("Search", &reader.search)
            .on_input(Message::SearchChanged)
            .on_submit(Message::SearchSubmit)
            .padding(6),
    ]
    .spacing(8)
    .padding(10);

    let mut items = column![].spacing(1);
    for item in &reader.items {
        items = items.push(item_row(item, reader));
    }
    if reader.items.is_empty() && !reader.loading {
        items = items.push(
            container(text("Nothing here.").style(muted))
                .padding(20)
                .center_x(Fill),
        );
    }
    if reader.loading {
        items = items.push(container(text("Loading…").style(muted)).padding(12));
    } else if reader.next_cursor.is_some() {
        items = items.push(
            container(button(text("Load more")).on_press(Message::LoadMore))
                .padding(12)
                .center_x(Fill),
        );
    }

    column![header, rule::horizontal(1), scrollable(items).height(Fill)]
        .width(LIST)
        .into()
}

fn item_row<'a>(item: &'a Item, reader: &'a Reader) -> Element<'a, Message> {
    let selected = reader.selected == Some(item.id);
    let unread = !item.read;
    let mut meta = vec![reader.feed_title(&item.feed_id)];
    meta.push(date(item.published));
    let title = text(&item.title)
        .size(14)
        .font(if unread {
            Font {
                weight: iced::font::Weight::Bold,
                ..Font::DEFAULT
            }
        } else {
            Font::DEFAULT
        })
        .style(move |theme: &Theme| text::Style {
            color: Some(if unread {
                Colors::of(theme).text
            } else {
                Colors::of(theme).muted
            }),
        });
    let mut top = row![title.width(Fill)].spacing(6);
    if item.starred {
        top = top.push(text("★").size(14).style(star));
    }
    button(column![top, text(meta.join(" · ")).size(12).style(muted)].spacing(2))
        .width(Fill)
        .padding([8, 10])
        .style(move |theme, status| row_style(theme, status, selected))
        .on_press(Message::OpenItem(item.id))
        .into()
}

fn article(reader: &Reader) -> Element<'_, Message> {
    let Some(item) = &reader.article else {
        return container(text("Select an item to read it.").style(muted))
            .center(Fill)
            .into();
    };

    let mut meta = vec![reader.feed_title(&item.feed_id)];
    if let Some(author) = &item.author {
        meta.push(author.clone());
    }
    meta.push(date(item.published));

    let actions = row![
        button(
            text(if item.starred {
                "★ Starred"
            } else {
                "☆ Star"
            })
            .size(13)
        )
        .style(button::secondary)
        .on_press(Message::Toggle(Flag::Starred)),
        button(
            text(if item.read {
                "Mark unread"
            } else {
                "Mark read"
            })
            .size(13)
        )
        .style(button::secondary)
        .on_press(Message::Toggle(Flag::Read)),
        button(text("Open in browser").size(13))
            .style(button::secondary)
            .on_press_maybe(item.url.as_ref().map(|_| Message::OpenInBrowser)),
    ]
    .spacing(8);

    let body = column![
        text(&item.title).size(24),
        text(meta.join(" · ")).size(13).style(muted),
        actions,
        rule::horizontal(1),
        text(&reader.article_text).size(15).line_height(1.5),
    ]
    .spacing(12)
    .padding(24)
    .max_width(760);

    scrollable(container(body).center_x(Fill))
        .width(Length::Fill)
        .height(Fill)
        .into()
}

fn date(unix: i64) -> String {
    match Local.timestamp_opt(unix, 0).single() {
        Some(t) if t.date_naive() == Local::now().date_naive() => t.format("%H:%M").to_string(),
        Some(t) => t.format("%b %-d, %Y").to_string(),
        None => String::new(),
    }
}

fn row_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let palette = theme.extended_palette();
    let background = match (selected, status) {
        (true, _) => Some(palette.background.strong.color.into()),
        (false, button::Status::Hovered) => Some(palette.background.weak.color.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: Colors::of(theme).text,
        border: iced::border::rounded(4),
        ..button::Style::default()
    }
}

fn background(theme: &Theme) -> container::Style {
    container::Style::default().background(Colors::of(theme).background)
}

fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Colors::of(theme).muted),
    }
}

fn error_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Colors::of(theme).error),
    }
}

fn star(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Colors::of(theme).star),
    }
}

#[cfg(test)]
mod tests {
    use iced_test::simulator;
    use pressbox_api::{Feed, Me, Version};

    use super::*;
    use crate::api::Client;

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

    fn item(id: i64, read: bool, starred: bool) -> Item {
        Item {
            id,
            feed_id: "a".into(),
            title: format!("Story {id}"),
            url: Some(format!("https://a.example/{id}")),
            author: Some("Ana".into()),
            summary: None,
            content: None,
            published: 1_793_700_000,
            read,
            starred,
        }
    }

    /// A reader with feeds (one failing, one archived) and two items.
    fn reader() -> Reader {
        let mut reader = Reader::new(Client::new("https://rss.example", Some("t".into())).unwrap());
        let mut failing = feed("b", 0, &[]);
        failing.last_error = Some("HTTP 500".into());
        let mut old = feed("old", 0, &[]);
        old.archived = true;
        reader.feeds = vec![feed("a", 3, &["tech"]), failing, old];
        reader.items = vec![item(1, false, true), item(2, true, false)];
        reader
    }

    #[test]
    fn setup_asks_for_a_server_and_shows_the_code() {
        let mut setup = Setup {
            server: "https://rss.example".into(),
            device: "laptop".into(),
            ..Setup::default()
        };
        let mut ui = simulator(setup_view(&setup));
        assert!(
            ui.find("Pair this computer with your pressbox server.")
                .is_ok()
        );
        assert!(ui.find("Approve this code:").is_err());
        ui.click("Pair this device").unwrap();
        assert!(matches!(
            ui.into_messages().collect::<Vec<_>>()[..],
            [Message::Pair]
        ));

        setup.busy = true;
        setup.code = Some("K7W-4QX".into());
        setup.error = Some("could not connect".into());
        let mut ui = simulator(setup_view(&setup));
        for shown in [
            "Approve this code:",
            "K7W-4QX",
            "Waiting for approval…",
            "could not connect",
        ] {
            assert!(ui.find(shown).is_ok(), "{shown}");
        }
        let _ = ui.click("Pair this device");
        assert_eq!(ui.into_messages().count(), 0, "disabled while pairing");
    }

    #[test]
    fn sidebar_lists_sources_and_who_we_are() {
        let mut reader = reader();
        reader.me = Some(Me {
            user: "chris".into(),
            admin: true,
            device: Some("laptop".into()),
        });
        reader.version = Some(Version {
            version: "0.1.0".into(),
            api: 1,
        });
        let mut ui = simulator(sidebar(&reader));
        for shown in [
            "Unread",
            "Tags",
            "#tech",
            "Feeds",
            "Feed a",
            "Feed b ⚠",
            "Archived",
            "Feed old",
        ] {
            assert!(ui.find(shown).is_ok(), "{shown}");
        }
        assert!(
            ui.find("https://rss.example\nchris on laptop\npressboxd 0.1.0")
                .is_ok()
        );
        ui.click("Starred").unwrap();
        ui.click("#tech").unwrap();
        ui.click("Refresh").unwrap();
        ui.click("Sign out").unwrap();
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(
            &messages[..],
            [
                Message::Select(Source::Starred),
                Message::Select(Source::Tag(tag)),
                Message::Refresh,
                Message::SignOut
            ] if tag == "tech"
        ));

        reader.me.as_mut().unwrap().device = None;
        assert_eq!(
            identity(&reader),
            "https://rss.example\nchris\npressboxd 0.1.0"
        );
    }

    #[test]
    fn item_list_opens_items_and_pages() {
        let mut reader = reader();
        reader.next_cursor = Some("c".into());
        let mut ui = simulator(item_list(&reader));
        assert!(ui.find("Unread").is_ok(), "the source is the heading");
        ui.click("Story 2").unwrap();
        ui.click("Load more").unwrap();
        ui.click("Mark all read").unwrap();
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(
            messages[..],
            [
                Message::OpenItem(2),
                Message::LoadMore,
                Message::MarkAllRead
            ]
        ));

        reader.items.clear();
        reader.source = Source::Starred;
        let mut ui = simulator(item_list(&reader));
        assert!(ui.find("Nothing here.").is_ok());
        let _ = ui.click("Mark all read");
        assert_eq!(
            ui.into_messages().count(),
            0,
            "starred lists can't be marked read"
        );
        reader.loading = true;
        let mut ui = simulator(item_list(&reader));
        assert!(ui.find("Loading…").is_ok() && ui.find("Nothing here.").is_err());
    }

    #[test]
    fn article_shows_the_item_and_its_actions() {
        let mut reader = reader();
        assert!(
            simulator(article(&reader))
                .find("Select an item to read it.")
                .is_ok()
        );

        reader.article = Some(item(1, false, true));
        reader.article_text = "The whole story.".into();
        let mut ui = simulator(article(&reader));
        assert!(ui.find("Story 1").is_ok() && ui.find("The whole story.").is_ok());
        ui.click("★ Starred").unwrap();
        ui.click("Mark read").unwrap();
        ui.click("Open in browser").unwrap();
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(matches!(
            messages[..],
            [
                Message::Toggle(Flag::Starred),
                Message::Toggle(Flag::Read),
                Message::OpenInBrowser
            ]
        ));

        let mut read = item(1, true, false);
        read.url = None;
        reader.article = Some(read);
        let mut ui = simulator(article(&reader));
        assert!(ui.find("☆ Star").is_ok() && ui.find("Mark unread").is_ok());
        let _ = ui.click("Open in browser");
        assert_eq!(ui.into_messages().count(), 0, "no link, nothing to open");
    }

    #[test]
    fn errors_and_status_show_in_a_banner() {
        let mut reader = reader();
        assert!(banner(&reader).is_none());
        reader.status = Some("Refreshing feeds…".into());
        assert!(
            simulator(banner(&reader).unwrap())
                .find("Refreshing feeds…")
                .is_ok()
        );
        reader.error = Some("offline".into());
        let mut ui = simulator(banner(&reader).unwrap());
        assert!(ui.find("offline").is_ok() && ui.find("Refreshing feeds…").is_err());
        ui.click("Dismiss").unwrap();
        assert!(matches!(
            ui.into_messages().collect::<Vec<_>>()[..],
            [Message::Dismiss]
        ));
    }

    #[test]
    fn whole_screens_render_in_both_themes() {
        let mut reader = reader();
        reader.selected = Some(1);
        reader.article = Some(item(1, false, true));
        reader.error = Some("offline".into());
        let reading = App {
            config: crate::config::ClientConfig::default(),
            config_path: None,
            screen: Screen::Reader(Box::new(reader)),
            dark: true,
        };
        let pairing = App {
            config: crate::config::ClientConfig::default(),
            config_path: None,
            screen: Screen::Setup(Setup::default()),
            dark: false,
        };
        for app in [&reading, &pairing] {
            for dark in [true, false] {
                let theme = if dark {
                    crate::theme::DARK.theme(true)
                } else {
                    crate::theme::LIGHT.theme(false)
                };
                // Drawing runs every style function, in both palettes.
                simulator(view(app)).snapshot(&theme).unwrap();
            }
        }
    }

    #[test]
    fn rows_show_selection_and_hover() {
        let theme = crate::theme::DARK.theme(true);
        let palette = theme.extended_palette();
        let background = |selected, status| row_style(&theme, status, selected).background;
        assert_eq!(
            background(true, button::Status::Active),
            Some(palette.background.strong.color.into())
        );
        assert_eq!(
            background(false, button::Status::Hovered),
            Some(palette.background.weak.color.into())
        );
        assert_eq!(background(false, button::Status::Active), None);
        let colors = Colors::of(&theme);
        assert_eq!(muted(&theme).color, Some(colors.muted));
        assert_eq!(error_text(&theme).color, Some(colors.error));
        assert_eq!(star(&theme).color, Some(colors.star));
    }

    #[test]
    fn dates_show_the_time_today() {
        let now = chrono::Utc::now().timestamp();
        assert_eq!(date(now).len(), 5, "like 14:05");
        assert!(date(1_793_700_000).ends_with(", 2026"));
        assert_eq!(date(i64::MAX), "");
    }
}
