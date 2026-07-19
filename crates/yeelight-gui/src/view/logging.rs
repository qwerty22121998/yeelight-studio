//! Command-log pane: raw wire traffic (sent + received) with a manager toolbar.

use iced::widget::{button, column, container, pick_list, row, scrollable, text};
use iced::{Element, Font, Length::Fill};
use yeelight_core::Direction;

use crate::app::{fmt_time, App};
use crate::message::{Message, Screen};

/// Render the command-log pane: a manager toolbar over the (newest-first) log list.
pub(crate) fn pane(app: &App) -> Element<'_, Message> {
    let back = button(text("\u{2190} Devices"))
        .style(button::text)
        .on_press(Message::SelectScreen(Screen::Device));

    let pause = button(text(if app.log_paused { "Resume" } else { "Pause" }))
        .on_press(Message::LogTogglePause);
    let clear = button(text("Clear")).on_press(Message::LogClear);
    let open = button(text("Open log file")).on_press(Message::LogOpenFile);

    // Device filter: "All" plus every known device id.
    let ids: Vec<String> = std::iter::once("All".to_string())
        .chain(app.devices.iter().map(|d| d.id.clone()))
        .collect();
    let selected = app.log_filter.clone().unwrap_or_else(|| "All".to_string());
    let filter = pick_list(ids, Some(selected), |s: String| {
        Message::LogFilterDevice((s != "All").then_some(s))
    });

    let toolbar = row![back, text("Command Log").size(22), pause, clear, open, filter]
        .spacing(10)
        .align_y(iced::Center);

    // Newest first. ponytail: reverse iteration, no scroll-to-bottom bookkeeping.
    let mut list = column![].spacing(2);
    let mut shown = 0usize;
    for e in app.logs.iter().rev() {
        if let Some(f) = &app.log_filter
            && &e.device != f
        {
            continue;
        }
        let (arrow, color) = match e.direction {
            Direction::Sent => ("\u{2192}", crate::theme::accent()),
            Direction::Received => ("\u{2190}", crate::theme::success()),
        };
        let row_text = format!("{} {arrow} {} {}", fmt_time(e.time), e.device, e.line);
        list = list.push(text(row_text).font(Font::MONOSPACE).size(12).color(color));
        shown += 1;
    }

    let count = text(format!("{shown} shown / {} captured", app.logs.len())).size(12);

    container(column![toolbar, count, scrollable(list).height(Fill)].spacing(12))
        .padding(20)
        .width(Fill)
        .height(Fill)
        .into()
}
