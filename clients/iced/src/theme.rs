// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Birdland colors, from https://github.com/chrisppy/base24-themes: Birdland
//! for dark mode and Birdland Light for light mode, the same values the iOS
//! app uses. The comments give each color's Base24 slot.

use iced::{Color, Theme, theme::Palette};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    /// base00: window and list backgrounds.
    pub background: Color,
    /// base05: primary text.
    pub text: Color,
    /// base04: secondary text.
    pub muted: Color,
    /// base08: Orioles orange, the accent.
    pub accent: Color,
    /// base12: errors.
    pub error: Color,
    /// base0A: stars.
    pub star: Color,
    /// base0B: read state.
    pub read: Color,
    /// base0D: links.
    pub link: Color,
}

pub const DARK: Colors = Colors {
    background: rgb(0x0B0B0B),
    text: rgb(0xE5DFD2),
    muted: rgb(0x8A8A8A),
    accent: rgb(0xDF4601),
    error: rgb(0xFF5A00),
    star: rgb(0xD4AD4C),
    read: rgb(0x4F8F6A),
    link: rgb(0xD98B45),
};

pub const LIGHT: Colors = Colors {
    background: rgb(0xF4EFE4),
    text: rgb(0x26231F),
    muted: rgb(0x6B665E),
    accent: rgb(0xB83C00),
    error: rgb(0xA83300),
    star: rgb(0x7A611D),
    read: rgb(0x3B7048),
    link: rgb(0x7F5530),
};

impl Colors {
    pub fn theme(&self, dark: bool) -> Theme {
        Theme::custom(
            if dark { "Birdland" } else { "Birdland Light" },
            Palette {
                background: self.background,
                text: self.text,
                primary: self.accent,
                success: self.read,
                warning: self.star,
                danger: self.error,
            },
        )
    }

    pub fn of(theme: &Theme) -> &'static Colors {
        if theme.extended_palette().is_dark {
            &DARK
        } else {
            &LIGHT
        }
    }
}

pub(crate) const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
