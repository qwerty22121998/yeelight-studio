//! View layer: pure `fn(&App) -> Element` builders, split by screen region.

pub(crate) mod components;
pub(crate) mod detail;
pub(crate) mod logging;
pub(crate) mod rail;
pub(crate) mod settings;

use iced::widget::{column, container, progress_bar, row, text};
use iced::{Element, Length::Fill};

use crate::app::{App, Status};
use crate::message::{Message, Screen};

/// The whole window: rail + detail/settings over a status bar.
pub(crate) fn root(app: &App) -> Element<'_, Message> {
    let content = match app.screen {
        Screen::Device => detail::pane(app),
        Screen::Settings => settings::pane(app),
        Screen::Logging => logging::pane(app),
    };
    let body = row![rail::view(app), content].height(Fill);
    column![body, status_bar(app)].into()
}

/// Slim bottom status bar: scan progress, or the last action / error.
fn status_bar(app: &App) -> Element<'_, Message> {
    let middle: Element<'_, Message> = if app.scanning {
        row![
            progress_bar(0.0..=1.0, app.scan_progress).length(200),
            text("scanning\u{2026}"),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into()
    } else {
        match &app.status {
            Status::Idle => text("ready").into(),
            Status::Ok(s) => text(s.clone()).into(),
            Status::Err(e) => text(format!("error: {e}"))
                .color(crate::theme::danger())
                .into(),
        }
    };
    container(middle).padding(10).width(Fill).into()
}
