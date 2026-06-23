//! Scenes tab: a grid of one-tap preset scenes.

use iced::widget::{button, column, row, text};
use iced::{Element, Length::Fill};
use yeelight_core::Device;

use crate::app::App;
use crate::message::Message;
use crate::presets::SCENES;

/// Render the Scenes tab body: preset scenes laid out three per row.
pub(crate) fn body<'a>(_app: &'a App, _d: &'a Device) -> Element<'a, Message> {
    let mut col = column![].spacing(8);
    let mut iter = SCENES.iter().enumerate().peekable();
    while iter.peek().is_some() {
        let mut r = row![].spacing(8);
        for (i, p) in iter.by_ref().take(3) {
            r = r.push(button(text(p.name)).width(Fill).on_press(Message::ApplyScene(i)));
        }
        col = col.push(r);
    }
    col.into()
}
