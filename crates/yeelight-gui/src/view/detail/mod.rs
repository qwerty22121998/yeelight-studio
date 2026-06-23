//! Detail pane: a device header over per-light control sections. The main light
//! gets the full tabbed surface; the background light (when supported) gets its
//! own clearly separated section with power, brightness, and color/white.

pub(crate) mod color;
pub(crate) mod flow;
pub(crate) mod music;
pub(crate) mod scenes;
pub(crate) mod timer;
pub(crate) mod white;

use iced::widget::{button, column, container, row, scrollable, slider, text, text_input, Space};
use iced::{Border, Color, Element, Length::Fill, Theme};
use yeelight_core::Device;

use super::components::{swatch, tab_strip};
use crate::app::{u32_to_color, App};
use crate::message::{CmdKind, DetailTab, Message};

/// Tabs shown for the main light.
const TABS: &[(&str, DetailTab)] = &[
    ("Color", DetailTab::Color),
    ("White", DetailTab::White),
    ("Scenes", DetailTab::Scenes),
    ("Flow", DetailTab::Flow),
    ("Timer", DetailTab::Timer),
    ("\u{26a1} Music", DetailTab::Music),
];

/// Render the detail pane for the selected device.
pub(crate) fn pane(app: &App) -> Element<'_, Message> {
    if app.devices.is_empty() {
        return container(text("No devices. Press Scan to discover bulbs on the LAN."))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into();
    }
    let Some(d) = app.selected.and_then(|i| app.devices.get(i)) else {
        return container(text("Select a device."))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into();
    };

    let mut col = column![header(app, d), main_section(app, d)].spacing(16);
    if bg_supported(app, d) {
        col = col.push(bg_section(app, d));
    }

    container(scrollable(col))
        .padding(16)
        .width(Fill)
        .height(Fill)
        .into()
}

/// Whether this device advertises any background-light method (or force-all is on).
fn bg_supported(app: &App, d: &Device) -> bool {
    app.force_all
        || ["bg_set_power", "bg_toggle", "bg_set_rgb", "bg_set_bright", "bg_set_ct_abx"]
            .iter()
            .any(|m| d.supports(m))
}

/// Device header: editable name, subtitle, and save-as-default.
fn header<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let name: Element<'a, Message> = match &app.rename {
        Some((id, buf)) if *id == d.id => row![
            text_input("name", buf)
                .on_input(Message::RenameEdit)
                .on_submit(Message::RenameCommit)
                .width(180),
            button(text("\u{2713}")).on_press(Message::RenameCommit),
            button(text("\u{2715}")).on_press(Message::RenameCancel),
        ]
        .spacing(6)
        .into(),
        _ => row![
            text(label_for(d)).size(22),
            button(text("\u{270e}"))
                .style(button::text)
                .on_press(Message::RenameStart),
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
    };

    let online = app.clients.contains_key(&d.id) || d.state.power.is_some();
    let sub = format!(
        "{} \u{b7} {} \u{b7} {}",
        String::from(d.model.clone()),
        d.location.ip(),
        if online { "online" } else { "offline" }
    );

    let save = button(text("Save as default"))
        .style(button::text)
        .on_press(Message::SaveDefault);

    row![
        column![name, text(sub).size(12).color(Color::from_rgb(0.55, 0.58, 0.63))].spacing(2),
        Space::new().width(Fill),
        save,
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// Main light: power (reflects live state) + brightness + the full tab surface.
fn main_section<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let on = d.state.power.unwrap_or(false);
    let power = button(text(if on { "\u{23fb} On" } else { "\u{23fc} Off" }))
        .style(if on { button::primary } else { button::secondary })
        .on_press(Message::Command { bg: false, kind: CmdKind::Toggle });

    let preview = d.state.rgb.map(u32_to_color).unwrap_or(Color::from_rgb(0.85, 0.8, 0.6));
    let tab = app.active_tab();
    let body: Element<'a, Message> = match tab {
        DetailTab::Color => color::body(app, d, false),
        DetailTab::White => white::body(app, d, false),
        DetailTab::Scenes => scenes::body(app, d),
        DetailTab::Flow => flow::body(app, d),
        DetailTab::Timer => timer::body(app, d),
        DetailTab::Music => music::body(app, d),
    };

    section_box(column![
        heading("Main light", power),
        row![swatch(preview, 48.0), brightness(app, d, false)].spacing(12).align_y(iced::Center),
        tab_strip(TABS, tab, Message::SelectDetailTab),
        body,
    ]
    .spacing(12))
}

/// Background light: its own power + brightness + color/white. No live power
/// state exists for the background light, so its button is a plain toggle.
fn bg_section<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let power = button(text("\u{23fb} Toggle"))
        .style(button::secondary)
        .on_press(Message::Command { bg: true, kind: CmdKind::Toggle });

    let preview = app
        .pickers
        .get(&d.id)
        .map(|p| p.bg_draft)
        .unwrap_or(Color::from_rgb(0.85, 0.8, 0.6));

    section_box(column![
        heading("Background light", power),
        row![swatch(preview, 48.0), brightness(app, d, true)].spacing(12).align_y(iced::Center),
        white::body(app, d, true),
        color::body(app, d, true),
    ]
    .spacing(12))
}

/// A section title with a power control pinned to the right.
fn heading<'a>(title: &'a str, power: iced::widget::Button<'a, Message>) -> Element<'a, Message> {
    row![text(title).size(16), Space::new().width(Fill), power]
        .spacing(10)
        .align_y(iced::Center)
        .into()
}

/// A brightness slider for the given light: drag updates the draft, release sends.
fn brightness<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let value = app
        .pickers
        .get(&d.id)
        .map(|p| if bg { p.bg_bright } else { p.main_bright })
        .unwrap_or_else(|| if bg { 100 } else { d.state.bright.unwrap_or(100) });
    row![
        text(format!("Brightness {value}%")).width(120),
        slider(1..=100u8, value, move |v| Message::BrightDraft { bg, value: v })
            .on_release(Message::Command { bg, kind: CmdKind::SetBright(value) }),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// Wrap a section's controls in a bordered box so main/background are visually distinct.
fn section_box<'a>(inner: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(inner)
        .padding(12)
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                border: Border {
                    color: p.background.strong.color,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

/// A short label: device name if set, else model + short id.
pub(crate) fn label_for(d: &Device) -> String {
    if let Some(name) = &d.state.name
        && !name.is_empty()
    {
        return name.clone();
    }
    let model = String::from(d.model.clone());
    let short = d.id.rsplit(':').next().unwrap_or(&d.id);
    let short = &short[short.len().saturating_sub(6)..];
    format!("{model} {short}")
}
