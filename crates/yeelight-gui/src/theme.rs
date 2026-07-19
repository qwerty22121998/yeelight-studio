//! Ember Dark — the app's default theme and semantic accent colors.
//!
//! A flat, warm-dark palette modeled on Impactor's "Plume Dark", with the accent
//! re-hued from mauve to orange (see `docs/ui-theme-guideline.md` for the full
//! spec and migration map). Supplying this as the [`iced::Theme`] re-hues every
//! built-in-styled widget for free; the standalone [`accent`]/[`success`]/
//! [`danger`]/[`muted`] colors cover the handful of text accents that sit outside
//! a style closure and can't read the theme.

use std::sync::OnceLock;

use iced::{Color, Theme};

use crate::message::ThemePref;

/// Display name of the Ember Dark theme — its pick-list label and on-disk key.
pub(crate) const EMBER_NAME: &str = "Ember Dark";

/// The Ember Dark theme: orange accent on warm near-black. Cached so every call
/// shares one `Arc`, keeping [`Theme`] equality (used by the theme pick-list and
/// settings round-trip) stable and pointer-cheap.
pub(crate) fn ember_dark() -> Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME
        .get_or_init(|| {
            Theme::custom(
                EMBER_NAME.to_string(),
                iced::theme::Palette {
                    background: Color::from_rgb8(0x26, 0x20, 0x1a),
                    text: Color::from_rgb8(0xf2, 0xe0, 0xcf),
                    primary: Color::from_rgb8(0xfe, 0x80, 0x19),
                    success: Color::from_rgb8(0xa9, 0xb6, 0x65),
                    warning: Color::from_rgb8(0xe9, 0xb1, 0x43),
                    danger: Color::from_rgb8(0xea, 0x69, 0x62),
                },
            )
        })
        .clone()
}

/// The default theme preference: Ember Dark.
pub(crate) fn default_pref() -> ThemePref {
    ThemePref::Fixed(ember_dark())
}

/// The orange accent — links, sent log lines, live-color hints.
pub(crate) fn accent() -> Color {
    Color::from_rgb8(0xfe, 0x80, 0x19)
}

/// Muted warm-grey for secondary text: subtitles, offline rows, placeholders.
pub(crate) fn muted() -> Color {
    Color::from_rgb8(0xb2, 0xa0, 0x8f)
}

/// "Online / ok / received" accent.
pub(crate) fn success() -> Color {
    Color::from_rgb8(0xa9, 0xb6, 0x65)
}

/// "Error" accent.
pub(crate) fn danger() -> Color {
    Color::from_rgb8(0xea, 0x69, 0x62)
}
