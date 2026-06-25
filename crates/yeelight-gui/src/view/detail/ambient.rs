//! Ambient section: pick a screen region + extraction mode + target light(s) (+ monitor when
//! more than one), then start streaming the screen's color to the bulb (music mode if
//! available, else rate-limited `set_rgb`). Device-wide — its own detail-pane section, not a
//! per-light tab, because one screen capture drives the main and/or background light.

use iced::widget::{button, checkbox, column, pick_list, row, text};
use iced::{Color, Element};
use yeelight_core::Device;

use super::color_modes;
use crate::ambient::color::{ExtractMode, Region};
use crate::app::App;
use crate::message::Message;

/// Render the Ambient section body (device-wide; main and/or background targets).
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let running = app.ambient.contains_key(&d.id);
    let cfg = app.ambient_cfg.get(&d.id).cloned().unwrap_or_default();

    let region = pick_list(Region::ALL, Some(cfg.region), Message::AmbientSetRegion);
    let mode = pick_list(ExtractMode::ALL, Some(cfg.mode), Message::AmbientSetMode);

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

    let mut col = column![
        text("\u{1f5b5} Ambient screen capture").size(16),
        status_line(app, d, running),
        row![text("Region").width(90), region].spacing(10).align_y(iced::Center),
        row![text("Mode").width(90), mode].spacing(10).align_y(iced::Center),
        targets,
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
        col = col.push(
            row![
                text("Monitor").width(90),
                pick_list(choices, Some(selected), |c| match c {
                    MonitorChoice::Primary => Message::AmbientSetMonitor(None),
                    MonitorChoice::Display(m) => Message::AmbientSetMonitor(Some(m.id)),
                }),
            ]
            .spacing(10)
            .align_y(iced::Center),
        );
    }

    let label = if running { "Stop ambient" } else { "Start ambient" };
    col.push(button(text(label)).on_press(Message::AmbientToggle)).into()
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

/// Running (with the live send mode) or off.
fn status_line<'a>(app: &App, d: &Device, running: bool) -> Element<'a, Message> {
    if running {
        let on_music = app.ambient.get(&d.id).map(|r| r.sink.music.is_some()).unwrap_or(false);
        let mode = if on_music { "music \u{b7} 15fps" } else { "fallback \u{b7} 2fps" };
        text(format!("Running ({mode}). Screen color is live."))
            .color(Color::from_rgb(0.3, 0.8, 0.5))
            .into()
    } else {
        text("Off. Start to mirror the screen's color onto the bulb.")
            .color(Color::from_rgb(0.55, 0.58, 0.63))
            .into()
    }
}
