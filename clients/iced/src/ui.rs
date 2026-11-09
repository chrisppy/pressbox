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
