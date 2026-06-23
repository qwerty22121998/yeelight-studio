//! White tab: color-temperature slider + named presets.

use iced::widget::{column, row, slider, text};
use iced::Element;
use yeelight_core::Device;

use super::super::components::chip;
use crate::app::App;
use crate::message::{CmdKind, Message};
use crate::presets::TEMPS;

/// Render the White tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let bg = app.target_light().is_bg();
    let value = app
        .pickers
        .get(&d.id)
        .map(|p| if bg { p.bg_ct } else { p.main_ct })
        .unwrap_or_else(|| if bg { 4000 } else { d.state.ct.unwrap_or(4000) });

    let sl = row![
        text(format!("{value} K")).width(90),
        slider(1700..=6500u16, value, move |v| Message::TempDraft { bg, value: v })
            .on_release(Message::Command { bg, kind: CmdKind::SetTemp(value) }),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let mut presets = row![].spacing(8);
    for (name, k) in TEMPS {
        presets = presets.push(chip(name, value == *k, Message::Command { bg, kind: CmdKind::SetTemp(*k) }));
    }

    column![sl, text("Presets").size(13), presets].spacing(12).into()
}
