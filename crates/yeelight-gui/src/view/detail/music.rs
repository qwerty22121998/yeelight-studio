//! Music tab: an "instant control" toggle. While on, color/brightness/temp
//! changes stream over the device's music channel — unthrottled and smooth.

use iced::widget::{button, column, text};
use iced::{Color, Element};
use yeelight_core::Device;

use crate::app::App;
use crate::message::Message;

/// Render the Music tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let on = app.music.contains_key(&d.id);
    let label = if on { "Disable instant mode" } else { "Enable instant mode" };
    let status = if on {
        text("Streaming — color/brightness/temp update live.").color(Color::from_rgb(0.3, 0.8, 0.5))
    } else {
        text("Off. Enable to stream control changes with no rate limit (smooth dragging).")
            .color(Color::from_rgb(0.55, 0.58, 0.63))
    };
    column![
        text("\u{26a1} Instant control (music mode)").size(16),
        status,
        button(text(label)).on_press(Message::MusicToggle),
    ]
    .spacing(12)
    .into()
}
