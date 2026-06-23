//! Left sidebar: switch between the Device and Settings screens.

use iced::widget::{button, column, container, text};
use iced::{Element, Length::Fill};

use crate::app::App;
use crate::message::{Message, Screen};

/// Render the sidebar with the active screen marked.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    let item = |label: &str, target: Screen| {
        let marker = if app.screen == target { "▶ " } else { "  " };
        button(text(format!("{marker}{label}")))
            .on_press(Message::SelectScreen(target))
            .width(Fill)
    };

    container(
        column![
            item("Device", Screen::Device),
            item("Settings", Screen::Settings),
        ]
        .spacing(6),
    )
    .padding(10)
    .width(150)
    .height(Fill)
    .into()
}
