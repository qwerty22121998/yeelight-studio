//! Device pane: a tab bar of discovered devices and the control screen for the
//! selected one. Each control is rendered only if the device advertises it.

use iced::widget::{button, column, container, row, scrollable, slider, text};
use iced::{Element, Length::Fill};
use iced_aw::ColorPicker;
use yeelight_core::Device;

use crate::app::{App, u32_to_color};
use crate::message::{Btn, CmdKind, Message};

/// Render the device tabs plus the selected device's controls.
pub(crate) fn pane(app: &App) -> Element<'_, Message> {
    if app.devices.is_empty() {
        return container(text("No devices. Press Scan to discover bulbs on the LAN."))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into();
    }

    let mut tabs = row![].spacing(6);
    for (i, d) in app.devices.iter().enumerate() {
        let marker = if app.selected == Some(i) { "● " } else { "" };
        tabs = tabs.push(
            button(text(format!("{marker}{}", label_for(d)))).on_press(Message::SelectTab(i)),
        );
    }

    let mut col = column![scrollable(tabs)].spacing(16);

    if let Some(d) = app.selected.and_then(|i| app.devices.get(i)) {
        col = col.push(text(label_for(d)).size(22));
        col = col.push(section(app, d, false));
        let bg_any = ["bg_set_power", "bg_toggle", "bg_set_rgb", "bg_set_bright", "bg_set_ct_abx"]
            .iter()
            .any(|m| d.supports(m));
        if app.force_all || bg_any {
            col = col.push(section(app, d, true));
        }
    }

    container(scrollable(col))
        .padding(16)
        .width(Fill)
        .height(Fill)
        .into()
}

/// One light section (main or background), with only the supported controls
/// (or all of them when [`App::force_all`] is set).
fn section<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let title = if bg { "Background light" } else { "Main light" };
    let mut col = column![text(title).size(18)].spacing(10);

    // `support` advertises one method name per capability; `force_all` ignores the set.
    let supports = |m: &str| app.force_all || d.supports(m);
    let m = |main: &'static str, back: &'static str| if bg { back } else { main };

    if supports(m("set_power", "bg_set_power")) || supports(m("toggle", "bg_toggle")) {
        let press = (!app.btn_busy(&d.id, bg, Btn::Toggle)).then_some(Message::Command {
            bg,
            kind: CmdKind::Toggle,
        });
        col = col.push(button(text("Toggle")).on_press_maybe(press));
    }

    if supports(m("set_bright", "bg_set_bright")) {
        col = col.push(bright_control(app, d, bg));
    }

    if supports(m("set_ct_abx", "bg_set_ct_abx")) {
        col = col.push(temp_control(app, d, bg));
    }

    if supports(m("set_rgb", "bg_set_rgb")) {
        col = col.push(color_control(app, d, bg));
    }

    container(col).padding(10).into()
}

/// Brightness slider (`1..=100`); drag updates the draft, release sends the command.
fn bright_control<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let ps = app.pickers.get(&d.id);
    let value = ps
        .map(|p| if bg { p.bg_bright } else { p.main_bright })
        // ponytail: first render falls back to last-known device brightness (main only).
        .unwrap_or_else(|| if bg { 100 } else { d.state.bright.unwrap_or(100) });
    let s = slider(1..=100u8, value, move |v| Message::BrightDraft { bg, value: v })
        .on_release(Message::Command { bg, kind: CmdKind::SetBright(value) });
    row![text(format!("Brightness: {value}%")).width(150), s]
        .spacing(10)
        .align_y(iced::Center)
        .into()
}

/// Color-temperature slider (`1700..=6500` K); drag updates the draft, release sends.
fn temp_control<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let ps = app.pickers.get(&d.id);
    let value = ps
        .map(|p| if bg { p.bg_ct } else { p.main_ct })
        .unwrap_or_else(|| if bg { 4000 } else { d.state.ct.unwrap_or(4000) });
    let s = slider(1700..=6500u16, value, move |v| Message::TempDraft { bg, value: v })
        .on_release(Message::Command { bg, kind: CmdKind::SetTemp(value) });
    row![text(format!("Temperature: {value}K")).width(150), s]
        .spacing(10)
        .align_y(iced::Center)
        .into()
}

/// The "Change color" button wrapped in the iced_aw color-picker overlay.
fn color_control<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let ps = app.pickers.get(&d.id);
    let open = ps.map(|p| if bg { p.bg_open } else { p.main_open }).unwrap_or(false);
    let draft = ps
        .map(|p| if bg { p.bg_draft } else { p.main_draft })
        .unwrap_or_else(|| d.state.rgb.map(u32_to_color).unwrap_or(iced::Color::WHITE));

    let press = (!app.btn_busy(&d.id, bg, Btn::Color)).then_some(Message::OpenPicker { bg });
    let button = button(text("Change color")).on_press_maybe(press);

    ColorPicker::new(
        open,
        draft,
        button,
        Message::CancelPicker { bg },
        move |color| Message::PickColor { bg, color },
    )
    .into()
}

/// A short tab/title label: device name if set, else model + short id.
fn label_for(d: &Device) -> String {
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
