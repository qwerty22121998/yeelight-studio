//! Detail pane: a device header over per-light control sections. The main light
//! gets the full tabbed surface; the background light (when supported) gets its
//! own clearly separated section with power, brightness, and color/white.

pub(crate) mod ambient;
pub(crate) mod audio;
pub(crate) mod color;
pub(crate) mod flow;
pub(crate) mod light;
pub(crate) mod music;
pub(crate) mod scenes;
pub(crate) mod timer;
pub(crate) mod white;

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Background, Border, Element, Length::Fill, Theme};
use yeelight_core::Device;

use super::components::{hex, swatch, tab_strip};
use super::dial::Dial;
use crate::app::App;
use crate::message::{CmdKind, DetailTab, Message, Section};

/// Control tabs in display order. Each shows only for a light that advertises
/// the matching method (see [`tab_supported`]) or when force-all is on.
const TABS: &[(&str, DetailTab)] = &[
    ("Light", DetailTab::Light),
    ("Scenes", DetailTab::Scenes),
    ("Flow", DetailTab::Flow),
    ("Timer", DetailTab::Timer),
    ("\u{266a} Music", DetailTab::Music),
];

/// Whether a control gated by `method` should be shown: the device advertises
/// it, or the user forced every control on.
pub(crate) fn enabled(app: &App, d: &Device, method: &str) -> bool {
    app.force_all || d.supports(method)
}

/// The `(rgb, ct)` color modes a light supports — gates which scenes/flows fit
/// (a temp-only light must not offer rgb presets, and vice-versa).
pub(crate) fn color_modes(app: &App, d: &Device, bg: bool) -> (bool, bool) {
    (
        enabled(app, d, if bg { "bg_set_rgb" } else { "set_rgb" }),
        enabled(app, d, if bg { "bg_set_ct_abx" } else { "set_ct_abx" }),
    )
}

/// Whether a light supporting `(rgb, ct)` can run a preset that needs `(rgb, ct)`.
pub(crate) fn fits((has_rgb, has_ct): (bool, bool), (needs_rgb, needs_ct): (bool, bool)) -> bool {
    (has_rgb || !needs_rgb) && (has_ct || !needs_ct)
}

/// Whether a tab is usable for the given light. Each feature maps to the method
/// the bulb must advertise — the `bg_*` twin for the background light. Timer
/// (`cron`) and Music have no background twin, so they are main-light only.
fn tab_supported(app: &App, d: &Device, tab: DetailTab, bg: bool) -> bool {
    let has = |main: &str, bgm: &str| enabled(app, d, if bg { bgm } else { main });
    match tab {
        DetailTab::Light => has("set_rgb", "bg_set_rgb") || has("set_ct_abx", "bg_set_ct_abx"),
        DetailTab::Scenes => has("set_scene", "bg_set_scene"),
        DetailTab::Flow => has("start_cf", "bg_start_cf"),
        DetailTab::Timer => !bg && enabled(app, d, "cron_add"),
        DetailTab::Music => !bg && enabled(app, d, "set_music"),
    }
}

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

    let mut col = column![header(app, d), light_section(app, d, false)].spacing(16);
    if bg_supported(app, d) {
        col = col.push(light_section(app, d, true));
    }
    // Ambient is device-wide (one screen capture → main and/or bg), so it gets its own
    // section rather than a per-light tab. Shown when either light advertises any color
    // control (rgb, or temperature for white-only bulbs).
    if ["set_rgb", "bg_set_rgb", "set_ct_abx", "bg_set_ct_abx"]
        .iter()
        .any(|m| enabled(app, d, m))
    {
        col = col.push(collapsible(
            app, d, Section::Ambient, "\u{25a3} Ambient screen capture", ambient::body,
        ));
        // Music-capture is likewise device-wide (one audio capture → main and/or bg).
        col = col.push(collapsible(
            app, d, Section::Audio, "\u{266b} Music capture", audio::body,
        ));
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
        || [
            "bg_set_power", "bg_toggle", "bg_set_rgb", "bg_set_bright", "bg_set_ct_abx",
            "bg_set_scene", "bg_start_cf",
        ]
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
            button(text("\u{2713}"))
                .style(crate::theme::primary_button)
                .on_press(Message::RenameCommit),
            button(text("\u{2715}"))
                .style(crate::theme::secondary_button)
                .on_press(Message::RenameCancel),
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

    let save: Element<'a, Message> = if enabled(app, d, "set_default") {
        button(text("Save as default"))
            .style(button::text)
            .on_press(Message::SaveDefault)
            .into()
    } else {
        Space::new().into()
    };

    row![
        column![name, text(sub).size(12).color(crate::theme::muted())].spacing(2),
        Space::new().width(Fill),
        save,
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// One light's controls — main or background, treated as peers: power, brightness,
/// and the tabs that light supports (`bg_*` methods gate the background light). The
/// device's reported power state was unreliable, so power is a plain toggle.
fn light_section<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let (title, toggle_m, power_m, bright_m) = if bg {
        ("Background light", "bg_toggle", "bg_set_power", "bg_set_bright")
    } else {
        ("Main light", "toggle", "set_power", "set_bright")
    };

    let power: Element<'a, Message> = if enabled(app, d, toggle_m) || enabled(app, d, power_m) {
        button(text("\u{23fb} Toggle"))
            .style(crate::theme::secondary_button)
            .on_press(Message::Command { bg, kind: CmdKind::Toggle })
            .into()
    } else {
        Space::new().into()
    };

    // Under programmatic control (ambient / music capture running and targeting this light),
    // the manual controls are suppressed — disabled *and* collapsed in one: a manual change
    // would just be overwritten. A static header (no chevron) replaces them; power stays live.
    let section = if bg { Section::Bg } else { Section::Main };
    let (amb, aud) = app.driving(&d.id, bg);
    if amb || aud {
        return section_box(
            column![section_header(title, true, None, power), driven_note(amb, aud)].spacing(12),
        );
    }

    // Otherwise the section is user-collapsible: a chevron header over the manual body.
    let collapsed = app.is_collapsed(&d.id, section);
    let header = section_header(title, collapsed, Some(Message::ToggleSection(section)), power);
    if collapsed {
        return section_box(header);
    }

    // Left rail: the light's current colour over its brightness dial — a compact,
    // always-visible status column. Shown only when the light actually dims.
    let rail: Option<Element<'a, Message>> = enabled(app, d, bright_m).then(|| {
        let color = light::preview(app, d, bg);
        column![
            swatch(color, 44.0),
            text(hex(color)).size(10).color(crate::theme::muted()),
            brightness(app, d, bg),
            text("Brightness").size(11).color(crate::theme::muted()),
        ]
        .spacing(8)
        .align_x(iced::Center)
        .into()
    });

    // Right body: the tab strip over the active tab's panel — only the tabs this
    // light supports (or all, when forced).
    let tabs: Vec<(&str, DetailTab)> = TABS
        .iter()
        .filter(|(_, t)| tab_supported(app, d, *t, bg))
        .map(|(label, tab)| (*label, *tab))
        .collect();
    let body: Option<Element<'a, Message>> = tabs.first().map(|&(_, first)| {
        // Keep the user's tab if still supported, else fall back to the first.
        let active = if tabs.iter().any(|(_, t)| *t == app.active_tab(bg)) {
            app.active_tab(bg)
        } else {
            first
        };
        let panel: Element<'a, Message> = match active {
            DetailTab::Light => light::body(app, d, bg),
            DetailTab::Scenes => scenes::body(app, d, bg),
            DetailTab::Flow => flow::body(app, d, bg),
            DetailTab::Timer => timer::body(app, d),
            DetailTab::Music => music::body(app, d),
        };
        column![
            tab_strip(&tabs, active, move |tab| Message::SelectDetailTab { bg, tab }),
            sub_panel(panel),
        ]
        .spacing(12)
        .into()
    });

    // Header over the rail + body split; either side may be absent.
    let content: Element<'a, Message> = match (rail, body) {
        (Some(r), Some(b)) => row![r, container(b).width(Fill)].spacing(18).into(),
        (Some(r), None) => r,
        (None, Some(b)) => b,
        (None, None) => Space::new().into(),
    };

    section_box(column![header, content].spacing(12))
}

/// A collapsible section header: a chevron + title (clickable to toggle when `toggle` is
/// `Some`) with an optional control pinned to the right. A `None` toggle renders a static
/// marker instead of a chevron — used when a section can't be expanded (driven light).
fn section_header<'a>(
    title: &'a str,
    collapsed: bool,
    toggle: Option<Message>,
    right: Element<'a, Message>,
) -> Element<'a, Message> {
    let glyph = match &toggle {
        Some(_) if collapsed => "\u{25b8}", // ▸
        Some(_) => "\u{25be}",              // ▾
        None => "\u{25aa}",                 // ▪ (static: not collapsible)
    };
    let label = row![
        text(glyph).size(13).color(crate::theme::muted()),
        text(title).size(16),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let head: Element<'a, Message> = match toggle {
        Some(msg) => button(label).style(button::text).padding(0).on_press(msg).into(),
        None => label.into(),
    };
    row![head, Space::new().width(Fill), right]
        .spacing(10)
        .align_y(iced::Center)
        .into()
}

/// The placeholder shown in a light section whose manual controls are suppressed because a
/// programmatic mode is driving it.
fn driven_note<'a>(amb: bool, aud: bool) -> Element<'a, Message> {
    let what = match (amb, aud) {
        (true, true) => "ambient + music capture",
        (true, false) => "ambient",
        _ => "music capture",
    };
    text(format!("Manual controls off \u{b7} driven by {what}"))
        .size(12)
        .color(crate::theme::muted())
        .into()
}

/// Wrap a device-wide section (`ambient` / `audio`) in the standard box with a collapsible
/// header; the body renders only when expanded.
fn collapsible<'a>(
    app: &'a App,
    d: &'a Device,
    section: Section,
    title: &'a str,
    body: fn(&'a App, &'a Device) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let collapsed = app.is_collapsed(&d.id, section);
    let header = section_header(title, collapsed, Some(Message::ToggleSection(section)), Space::new().into());
    if collapsed {
        section_box(header)
    } else {
        section_box(column![header, body(app, d)].spacing(12))
    }
}

/// A circular brightness dial for the given light: drag or scroll the arc; the
/// draft updates live and the command sends on release.
fn brightness<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let value = app
        .pickers
        .get(&d.id)
        .map(|p| if bg { p.bg_bright } else { p.main_bright })
        .unwrap_or_else(|| if bg { 100 } else { d.state.bright.unwrap_or(100) });
    Dial::new(
        value,
        move |v| Message::BrightDraft { bg, value: v },
        move |v| Message::Command { bg, kind: CmdKind::SetBright(v) },
    )
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
                    radius: crate::theme::RADIUS.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

/// Wrap a tab's body in a recessed, bordered panel so the selected sub-menu's
/// content reads as clearly nested under its tab strip.
fn sub_panel<'a>(inner: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(inner)
        .padding(12)
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(Background::Color(crate::theme::darken(p.background.base.color, 0.02))),
                border: Border {
                    color: p.background.strong.color,
                    width: 1.0,
                    radius: crate::theme::RADIUS.into(),
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
