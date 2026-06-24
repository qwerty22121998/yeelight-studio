//! Left rail: device list (status + live color) over Scan / Settings.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Color, Element, Length::Fill};

use super::components::swatch;
use super::detail::label_for;
use crate::app::{u32_to_color, App};
use crate::message::{Message, Screen};

/// Render the rail.
pub(crate) fn view(app: &App) -> Element<'_, Message> {
    let mut list = column![].spacing(4);
    for (i, d) in app.devices.iter().enumerate() {
        let selected = app.selected == Some(i) && app.screen == Screen::Device;
        let online = app.clients.contains_key(&d.id) || d.state.power.is_some();
        let dot = if online {
            Color::from_rgb(0.2, 0.83, 0.6)
        } else {
            Color::from_rgb(0.42, 0.45, 0.5)
        };
        let chip_color = d
            .state
            .rgb
            .map(u32_to_color)
            .unwrap_or(Color::from_rgb(0.5, 0.5, 0.5));
        let rowel = row![
            swatch(dot, 9.0),
            text(label_for(d)).width(Fill),
            swatch(chip_color, 14.0),
        ]
        .spacing(8)
        .align_y(iced::Center);
        let b = button(rowel)
            .width(Fill)
            .on_press(Message::SelectTab(i))
            .style(if selected {
                button::primary
            } else {
                button::text
            });
        list = list.push(b);
    }

    let scan = button(text("\u{ff0b} Scan"))
        .width(Fill)
        .on_press_maybe((!app.scanning).then_some(Message::Scan));
    let settings = button(text("\u{2699} Settings"))
        .width(Fill)
        .on_press(Message::SelectScreen(Screen::Settings));
    let logs = button(text("\u{1f4d1} Logs"))
        .width(Fill)
        .on_press(Message::SelectScreen(Screen::Logging));

    container(
        column![
            text("Yeelight Studio").size(16),
            scrollable(list).height(Fill),
            scan,
            settings,
            logs,
        ]
        .spacing(10),
    )
    .padding(12)
    .width(200)
    .height(Fill)
    .into()
}
