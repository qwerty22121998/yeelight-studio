//! Timer tab: a sleep timer (power off after N minutes) with a live countdown.

use iced::widget::{button, column, row, text, text_input};
use iced::Element;
use yeelight_core::Device;

use crate::app::App;
use crate::message::Message;

/// Render the Timer tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    if let Some(secs) = app.timers.get(&d.id).and_then(|t| t.remaining) {
        let (m, s) = (secs / 60, secs % 60);
        return column![
            text(format!("Powering off in {m:02}:{s:02}")).size(16),
            button(text("Cancel timer"))
                .style(crate::theme::secondary_button)
                .on_press(Message::TimerCancel),
        ]
        .spacing(12)
        .into();
    }
    let input = app.timer_input.get(&d.id).cloned().unwrap_or_default();
    row![
        text("Sleep after (min):"),
        text_input("30", &input)
            .on_input(Message::TimerEdit)
            .on_submit(Message::TimerStart)
            .width(80),
        button(text("Start"))
            .style(crate::theme::primary_button)
            .on_press(Message::TimerStart),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}
