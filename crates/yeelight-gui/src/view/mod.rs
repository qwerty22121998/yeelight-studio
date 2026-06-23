//! View layer: pure `fn(&App) -> Element` builders, split by screen region.

pub(crate) mod device;
pub(crate) mod settings;
pub(crate) mod sidebar;

use iced::widget::{button, column, container, progress_bar, row, text, Space};
use iced::{Element, Length::Fill};

use crate::app::{App, Status};
use crate::message::{Message, Screen};

/// The whole window: sidebar + content over a bottom action bar.
pub(crate) fn root(app: &App) -> Element<'_, Message> {
    let content = match app.screen {
        Screen::Device => device::pane(app),
        Screen::Settings => settings::pane(app),
    };

    let body = row![sidebar::view(app), content].height(Fill);

    column![body, bottom_bar(app)].into()
}

/// Scan + progress/status on the left, Quit pinned to the right.
fn bottom_bar(app: &App) -> Element<'_, Message> {
    // Scan is disabled only while a scan runs; Quit is always available.
    let scan = button(text("Scan")).on_press_maybe((!app.scanning).then_some(Message::Scan));

    let middle: Element<'_, Message> = if app.scanning {
        row![
            progress_bar(0.0..=1.0, app.scan_progress).length(200),
            text("scanning…"),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into()
    } else {
        match &app.status {
            Status::Idle => text("ready").into(),
            Status::Ok(s) => text(s.clone()).into(),
            Status::Err(e) => {
                text(format!("error: {e}")).color(iced::Color::from_rgb(0.9, 0.3, 0.3)).into()
            }
        }
    };

    container(
        row![
            scan,
            middle,
            Space::new().width(Fill),
            button(text("Quit")).on_press(Message::Quit),
        ]
        .spacing(10)
        .align_y(iced::Center),
    )
    .padding(10)
    .width(Fill)
    .into()
}
