//! Midnight Blue — the app's default theme and semantic accent colors.
//!
//! A flat, cool-dark palette modeled on Impactor's "Plume Dark", with the accent
//! re-hued to azure blue (see `docs/ui-theme-guideline.md` for the full
//! spec and migration map). Supplying this as the [`iced::Theme`] re-hues every
//! built-in-styled widget for free; the standalone [`accent`]/[`success`]/
//! [`danger`]/[`muted`] colors cover the handful of text accents that sit outside
//! a style closure and can't read the theme.

use std::sync::OnceLock;

use iced::widget::button;
use iced::widget::pick_list::{Status as PickStatus, Style as PickStyle};
use iced::{Background, Border, Color, Shadow, Theme};

use crate::message::ThemePref;

/// Uniform corner radius — Impactor uses 4px everywhere (buttons, panels, chips).
pub(crate) const RADIUS: f32 = 4.0;

/// Add `amount` to each RGB channel (clamped). Impactor's flat-surface elevation.
pub(crate) fn lighten(c: Color, amount: f32) -> Color {
    Color {
        r: (c.r + amount).min(1.0),
        g: (c.g + amount).min(1.0),
        b: (c.b + amount).min(1.0),
        a: c.a,
    }
}

/// Subtract `amount` from each RGB channel (clamped).
pub(crate) fn darken(c: Color, amount: f32) -> Color {
    Color {
        r: (c.r - amount).max(0.0),
        g: (c.g - amount).max(0.0),
        b: (c.b - amount).max(0.0),
        a: c.a,
    }
}

fn flat(bg: Color, text: Color) -> button::Style {
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: text,
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: RADIUS.into() },
        shadow: Shadow::default(),
        snap: false,
    }
}

/// Primary (accent) button: filled azure with dark text — the Impactor CTA look.
pub(crate) fn primary_button(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.palette();
    match status {
        button::Status::Active => flat(p.primary, p.background),
        button::Status::Hovered => flat(lighten(p.primary, 0.15), lighten(p.background, 0.1)),
        button::Status::Pressed => flat(lighten(p.primary, 0.03), darken(p.background, 0.1)),
        button::Status::Disabled => {
            flat(lighten(p.primary, 0.05).scale_alpha(0.2), p.background.scale_alpha(0.5))
        }
    }
}

/// Secondary button: a raised neutral surface — most buttons use this.
pub(crate) fn secondary_button(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.palette();
    match status {
        button::Status::Active => flat(lighten(p.background, 0.03), p.text),
        button::Status::Hovered => flat(lighten(p.background, 0.15), lighten(p.text, 0.1)),
        button::Status::Pressed => flat(lighten(p.background, 0.03), darken(p.text, 0.1)),
        button::Status::Disabled => flat(lighten(p.background, 0.05), p.text.scale_alpha(0.5)),
    }
}

/// Flat pick-list matching the secondary-button surface.
pub(crate) fn pick_list(theme: &Theme, status: PickStatus) -> PickStyle {
    let p = theme.palette();
    let bg = match status {
        PickStatus::Active => lighten(p.background, 0.03),
        PickStatus::Hovered | PickStatus::Opened { .. } => lighten(p.background, 0.12),
    };
    PickStyle {
        text_color: p.text,
        placeholder_color: muted(),
        handle_color: p.text,
        background: Background::Color(bg),
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: RADIUS.into() },
    }
}

/// Display name of the Midnight Blue theme — its pick-list label and on-disk key.
pub(crate) const EMBER_NAME: &str = "Midnight Blue";

/// The Midnight Blue theme: azure accent on cool near-black. Cached so every call
/// shares one `Arc`, keeping [`Theme`] equality (used by the theme pick-list and
/// settings round-trip) stable and pointer-cheap.
pub(crate) fn ember_dark() -> Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME
        .get_or_init(|| {
            Theme::custom(
                EMBER_NAME.to_string(),
                iced::theme::Palette {
                    background: Color::from_rgb8(0x10, 0x18, 0x2a),
                    text: Color::from_rgb8(0xd8, 0xe4, 0xf4),
                    primary: Color::from_rgb8(0x4d, 0x9d, 0xff),
                    success: Color::from_rgb8(0x6f, 0xc9, 0xa8),
                    warning: Color::from_rgb8(0xe9, 0xb1, 0x43),
                    danger: Color::from_rgb8(0xea, 0x69, 0x62),
                },
            )
        })
        .clone()
}

/// The default theme preference: Midnight Blue.
pub(crate) fn default_pref() -> ThemePref {
    ThemePref::Fixed(ember_dark())
}

/// The azure accent — links, sent log lines, live-color hints.
pub(crate) fn accent() -> Color {
    Color::from_rgb8(0x4d, 0x9d, 0xff)
}

/// Muted cool-grey for secondary text: subtitles, offline rows, placeholders.
pub(crate) fn muted() -> Color {
    Color::from_rgb8(0x89, 0x98, 0xb2)
}

/// "Online / ok / received" accent.
pub(crate) fn success() -> Color {
    Color::from_rgb8(0x6f, 0xc9, 0xa8)
}

/// "Error" accent.
pub(crate) fn danger() -> Color {
    Color::from_rgb8(0xea, 0x69, 0x62)
}
