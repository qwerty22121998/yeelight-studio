//! Timer tab (placeholder; filled in a later task).
use iced::widget::text;
use iced::Element;
use yeelight_core::Device;

use crate::app::App;
use crate::message::Message;

/// Render the Timer tab body.
pub(crate) fn body<'a>(_app: &'a App, _d: &'a Device) -> Element<'a, Message> {
    text("Timer").into()
}
