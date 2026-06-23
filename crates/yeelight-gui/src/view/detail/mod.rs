//! Detail pane: header + hero + tabbed controls for the selected device.

pub(crate) mod color;
pub(crate) mod flow;
pub(crate) mod music;
pub(crate) mod scenes;
pub(crate) mod timer;
pub(crate) mod white;

use iced::widget::{button, column, container, row, scrollable, slider, text, text_input, Space};
use iced::{Color, Element, Length::Fill};
use yeelight_core::Device;

use super::components::{segmented, swatch, tab_strip};
use crate::app::{u32_to_color, App};
use crate::message::{CmdKind, DetailTab, Light, Message};

/// Tabs shown across the detail pane for every device.
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

    let tab = app.active_tab();
    let body: Element<'_, Message> = match tab {
        DetailTab::Color => color::body(app, d),
        DetailTab::White => white::body(app, d),
        DetailTab::Scenes => scenes::body(app, d),
        DetailTab::Flow => flow::body(app, d),
        DetailTab::Timer => timer::body(app, d),
        DetailTab::Music => music::body(app, d),
    };

    container(
        column![
            header(app, d),
            hero(app, d),
            tab_strip(TABS, tab, Message::SelectDetailTab),
            scrollable(body).height(Fill),
        ]
        .spacing(14),
    )
    .padding(16)
    .width(Fill)
    .height(Fill)
    .into()
}

/// Name (editable) + subtitle + power switch + save-default action.
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

    let on = d.state.power.unwrap_or(false);
    let online = app.clients.contains_key(&d.id) || d.state.power.is_some();
    let sub = format!(
        "{} \u{b7} {} \u{b7} {}",
        String::from(d.model.clone()),
        d.location.ip(),
        if online { "online" } else { "offline" }
    );

    let power = button(text(if on { "\u{23fb} On" } else { "\u{23fc} Off" }))
        .style(if on { button::primary } else { button::secondary })
        .on_press(Message::Command { bg: false, kind: CmdKind::Toggle });

    let save = button(text("Save as default"))
        .style(button::text)
        .on_press(Message::SaveDefault);

    row![
        column![name, text(sub).size(12).color(Color::from_rgb(0.55, 0.58, 0.63))].spacing(2),
        Space::new().width(Fill),
        save,
        power,
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// Live color preview + brightness + Main/Background segment (when bg supported).
fn hero<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let light = app.target_light();
    let bg = light.is_bg();
    let preview = d.state.rgb.map(u32_to_color).unwrap_or(Color::from_rgb(0.85, 0.8, 0.6));

    let value = app
        .pickers
        .get(&d.id)
        .map(|p| if bg { p.bg_bright } else { p.main_bright })
        .unwrap_or_else(|| {
            if bg {
                100
            } else {
                d.state.bright.unwrap_or(100)
            }
        });
    let bright = row![
        text(format!("Brightness {value}%")).width(150),
        slider(1..=100u8, value, move |v| Message::BrightDraft { bg, value: v })
            .on_release(Message::Command { bg, kind: CmdKind::SetBright(value) }),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let bg_supported = app.force_all
        || ["bg_set_power", "bg_toggle", "bg_set_rgb", "bg_set_bright", "bg_set_ct_abx"]
            .iter()
            .any(|m| d.supports(m));

    let mut col = column![row![swatch(preview, 56.0)].spacing(10)].spacing(12);
    if bg_supported {
        col = col.push(segmented(
            ("Main", Message::SelectLight(Light::Main)),
            ("Background", Message::SelectLight(Light::Background)),
            !bg,
        ));
    }
    col = col.push(bright);
    col.into()
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
