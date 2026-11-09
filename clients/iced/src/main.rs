// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! pressbox-iced: a desktop client for pressbox.

mod api;
mod app;
mod config;
#[cfg(test)]
mod test_server;
mod theme;
mod ui;

// Not measured: opens a window; the app's state, views and tasks are
// tested without one.
#[cfg(not(tarpaulin_include))]
fn main() -> iced::Result {
    iced::application(app::App::boot, app::App::update, ui::view)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1200.0, 780.0),
            min_size: Some(iced::Size::new(760.0, 420.0)),
            // Matches the desktop file's name, so docks and Wayland
            // compositors (window rules in niri, say) can tell it apart.
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "pressbox-iced".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .run()
}
