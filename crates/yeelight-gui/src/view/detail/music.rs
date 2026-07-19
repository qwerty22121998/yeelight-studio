//! Music tab: an "instant control" toggle. While on, color/brightness/temp
//! changes stream over the device's music channel — unthrottled and smooth.

use iced::widget::{button, column, text};
use iced::Element;
use yeelight_core::Device;

use crate::app::App;
use crate::message::Message;

/// Render the Music tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let on = app.music.contains_key(&d.id);
    let label = if on { "Disable instant mode" } else { "Enable instant mode" };
    let status = if on {
        text("Streaming — color/brightness/temp update live.").color(crate::theme::success())
    } else {
        text("Off. Enable to stream control changes with no rate limit (smooth dragging).")
            .color(crate::theme::muted())
    };
    column![
        text("\u{266a} Instant control (music mode)").size(16),
        status,
        button(text(label))
            .style(crate::theme::primary_button)
            .on_press(Message::MusicToggle),
    ]
    .spacing(12)
    .into()
}
