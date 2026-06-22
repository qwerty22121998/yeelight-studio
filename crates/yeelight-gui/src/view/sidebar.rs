//! Left sidebar: switch between the Device and Setting panes.

use iced::widget::{button, column, container, text};
use iced::{Element, Length::Fill};

use crate::app::App;
use crate::message::{Message, Sidebar};

/// Render the sidebar with the active pane marked.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    let item = |label: &str, target: Sidebar| {
        let marker = if app.sidebar == target { "▶ " } else { "  " };
        button(text(format!("{marker}{label}")))
            .on_press(Message::SelectSidebar(target))
            .width(Fill)
    };

    container(
        column![
            item("Device", Sidebar::Device),
            item("Setting", Sidebar::Setting),
        ]
        .spacing(6),
    )
    .padding(10)
    .width(150)
    .height(Fill)
    .into()
}
