//! Ambient tab: pick a screen region + extraction mode + target light(s) (+ monitor when
//! more than one), then start streaming the screen's color to the bulb (music mode if
//! available, else rate-limited `set_rgb`).

use iced::widget::{button, checkbox, column, pick_list, row, text};
use iced::{Color, Element};
use yeelight_core::Device;

use super::enabled;
use crate::ambient::capture;
use crate::ambient::color::{ExtractMode, Region};
use crate::app::App;
use crate::message::Message;

/// Render the Ambient tab body (main-light surface only).
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let running = app.ambient.contains_key(&d.id);
    let cfg = app.ambient_cfg.get(&d.id).cloned().unwrap_or_default();

    let region = pick_list(Region::ALL, Some(cfg.region), Message::AmbientSetRegion);
    let mode = pick_list(ExtractMode::ALL, Some(cfg.mode), Message::AmbientSetMode);

    // Target checkboxes — each shown only if that light advertises RGB.
    let mut targets = column![].spacing(6);
    if enabled(app, d, "set_rgb") {
        targets = targets.push(
            checkbox(cfg.main)
                .label("Main light")
                .on_toggle(|on| Message::AmbientSetTarget { main: true, on }),
        );
    }
    if enabled(app, d, "bg_set_rgb") {
        targets = targets.push(
            checkbox(cfg.bg)
                .label("Background light")
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
    // monitor is fixed at start). Selecting one sets the start-time monitor.
    if !running {
        let monitors = capture::monitors();
        if monitors.len() > 1 {
            let selected = monitors.iter().find(|m| Some(m.id) == cfg.monitor_id).cloned();
            col = col.push(
                row![
                    text("Monitor").width(90),
                    pick_list(monitors, selected, |m| Message::AmbientSetMonitor(Some(m.id))),
                ]
                .spacing(10)
                .align_y(iced::Center),
            );
        }
    }

    let label = if running { "Stop ambient" } else { "Start ambient" };
    col.push(button(text(label)).on_press(Message::AmbientToggle)).into()
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
