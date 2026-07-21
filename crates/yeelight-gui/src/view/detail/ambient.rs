//! Ambient section: pick a screen region + extraction mode + target light(s) (+ monitor when
//! more than one), then start streaming the screen's color to the bulb (music mode if
//! available, else rate-limited `set_rgb`). Device-wide — its own detail-pane section, not a
//! per-light tab, because one screen capture drives the main and/or background light.

use iced::widget::{button, checkbox, column, container, pick_list, row, text};
use iced::{Background, Border, Color, Element, Length, Shadow, Theme};
use yeelight_core::Device;

use super::color_modes;
use crate::ambient::color::{ExtractMode, Region};
use crate::app::App;
use crate::message::Message;
use crate::view::components::chip;

/// Render the Ambient section body (device-wide; main and/or background targets).
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let running = app.ambient.contains_key(&d.id);
    let cfg = app.ambient_cfg.get(&d.id).cloned().unwrap_or_default();

    // Mode as a segmented control (matches the Music-capture section) — three fixed
    // extraction modes read better as always-visible chips than a dropdown.
    let mut mode = row![].spacing(4);
    for m in ExtractMode::ALL {
        mode = mode.push(chip(ext_label(m), m == cfg.mode, Message::AmbientSetMode(m)));
    }

    // Target checkboxes — each shown if that light has any color control. A target with
    // only temperature support (white-only bulb) is labelled so, since ambient will drive
    // it by warm/cool K rather than full color.
    let (main_rgb, main_ct) = color_modes(app, d, false);
    let (bg_rgb, bg_ct) = color_modes(app, d, true);
    let mut targets = column![].spacing(6);
    if main_rgb || main_ct {
        let label = if main_rgb { "Main light" } else { "Main light (temperature)" };
        targets = targets.push(
            checkbox(cfg.main)
                .label(label)
                .on_toggle(|on| Message::AmbientSetTarget { main: true, on }),
        );
    }
    if bg_rgb || bg_ct {
        let label = if bg_rgb { "Background light" } else { "Background light (temperature)" };
        targets = targets.push(
            checkbox(cfg.bg)
                .label(label)
                .on_toggle(|on| Message::AmbientSetTarget { main: false, on }),
        );
    }

    let smooth = checkbox(cfg.smooth)
        .label("Smooth transitions")
        .on_toggle(Message::AmbientSetSmooth);

    // Right column: capture settings — mode, target light(s), and options.
    let mut controls = column![
        row![text("Mode").width(90), mode].spacing(10).align_y(iced::Center),
        targets,
        smooth,
    ]
    .spacing(12);

    // Monitor picker: only when there's a choice, and only while stopped (the capture
    // monitor is fixed at start). Selecting one sets the start-time monitor. The list is
    // cached in App (enumerating spawns a subprocess) and refreshed on scan.
    if !running && app.monitors.len() > 1 {
        // Include an explicit "Primary (auto)" choice mapping to monitor_id = None, so the
        // default is selectable and shown (capture falls back to the focused/first display).
        let mut choices = vec![MonitorChoice::Primary];
        choices.extend(app.monitors.iter().cloned().map(MonitorChoice::Display));
        let selected = match cfg.monitor_id {
            Some(id) => app
                .monitors
                .iter()
                .find(|m| m.id == id)
                .cloned()
                .map_or(MonitorChoice::Primary, MonitorChoice::Display),
            None => MonitorChoice::Primary,
        };
        controls = controls.push(
            row![
                text("Monitor").width(90),
                pick_list(choices, Some(selected), |c| match c {
                    MonitorChoice::Primary => Message::AmbientSetMonitor(None),
                    MonitorChoice::Display(m) => Message::AmbientSetMonitor(Some(m.id)),
                })
                .style(crate::theme::pick_list),
            ]
            .spacing(10)
            .align_y(iced::Center),
        );
    }

    // Left rail: the region "screen" picker as the section's visual anchor — same
    // rail-plus-caption shape as the light sections' brightness dial.
    let rail = column![
        region_picker(cfg.region),
        text("Region").size(11).color(crate::theme::muted()),
    ]
    .spacing(8)
    .align_x(iced::Center);

    // Status over the rail/settings split, with Start as a full-width CTA. The section title
    // lives in the collapsible header (see `detail::collapsible`).
    let label = if running { "Stop ambient" } else { "Start ambient" };
    column![
        status_line(app, d, running),
        row![rail, container(controls).width(Length::Fill)].spacing(18),
        button(text(label))
            .style(crate::theme::primary_button)
            .on_press(Message::AmbientToggle),
    ]
    .spacing(12)
    .into()
}

/// Short label for an extraction mode's segment chip ("Average + saturation" shortens
/// to "Saturated" so the chip row stays compact).
fn ext_label(m: ExtractMode) -> &'static str {
    match m {
        ExtractMode::Average => "Average",
        ExtractMode::Dominant => "Dominant",
        ExtractMode::AverageSaturated => "Saturated",
    }
}

/// A monitor-picker choice: the auto/primary default (`monitor_id = None`) or a specific
/// display. Lets the picker show and re-select the default instead of only concrete monitors.
#[derive(Clone, PartialEq)]
enum MonitorChoice {
    Primary,
    Display(crate::ambient::capture::Monitor),
}

impl std::fmt::Display for MonitorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MonitorChoice::Primary => f.write_str("Primary (auto)"),
            MonitorChoice::Display(m) => write!(f, "{m}"),
        }
    }
}

/// A screen mock-up split into clickable zones — click an edge to sample that band, or the
/// centre for the whole screen. The selected zone is lit in accent. Edge zones are laid out
/// as a top/bottom band + a left/centre/right middle row; the actual crop still comes from
/// [`color::crop_bounds`] (`EDGE_FRACTION`), this just picks which [`Region`].
fn region_picker(current: Region) -> Element<'static, Message> {
    let zone = move |label: &'static str, region: Region| {
        let selected = region == current;
        button(text(label).size(11).width(Length::Fill).align_x(iced::Center))
            .padding(3)
            .on_press(Message::AmbientSetRegion(region))
            .style(move |theme, status| zone_style(theme, status, selected))
    };
    let mid = row![
        zone("Left", Region::Left).width(Length::FillPortion(1)).height(Length::Fill),
        zone("Full", Region::Whole).width(Length::FillPortion(2)).height(Length::Fill),
        zone("Right", Region::Right).width(Length::FillPortion(1)).height(Length::Fill),
    ]
    .spacing(2)
    .height(Length::FillPortion(2));
    let grid = column![
        zone("Top", Region::Top).width(Length::Fill).height(Length::FillPortion(1)),
        mid,
        zone("Bottom", Region::Bottom).width(Length::Fill).height(Length::FillPortion(1)),
    ]
    .spacing(2);
    container(grid)
        .width(Length::Fixed(200.0))
        .height(Length::Fixed(128.0))
        .padding(3)
        .style(screen_style)
        .into()
}

/// The picker's bezel: raised surface + muted 1px frame (the "screen").
fn screen_style(theme: &Theme) -> container::Style {
    let p = theme.palette();
    container::Style {
        background: Some(Background::Color(crate::theme::lighten(p.background, 0.03))),
        border: Border {
            color: crate::theme::muted(),
            width: 1.0,
            radius: crate::theme::RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// One picker zone: accent when selected, dark glass otherwise, lifting on hover.
fn zone_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let p = theme.palette();
    let base = if selected {
        crate::theme::accent()
    } else {
        crate::theme::darken(p.background, 0.1)
    };
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => crate::theme::lighten(base, 0.08),
        _ => base,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if selected { p.background } else { crate::theme::muted() },
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: 2.0.into() },
        shadow: Shadow::default(),
        snap: false,
    }
}

/// Running (with the live send mode) or off.
fn status_line<'a>(app: &App, d: &Device, running: bool) -> Element<'a, Message> {
    if running {
        let on_music = app.ambient.get(&d.id).map(|r| r.sink.music.is_some()).unwrap_or(false);
        let mode = if on_music { "music \u{b7} 15fps" } else { "fallback \u{b7} 2fps" };
        text(format!("Running ({mode}). Screen color is live."))
            .color(crate::theme::success())
            .into()
    } else {
        text("Off. Start to mirror the screen's color onto the bulb.")
            .color(crate::theme::muted())
            .into()
    }
}
