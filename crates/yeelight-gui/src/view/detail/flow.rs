//! Flow tab: preset flows + a custom flow editor.

use iced::widget::{button, column, pick_list, row, text, text_input, Space};
use iced::{Element, Length::Fill};
use yeelight_core::Device;

use crate::app::App;
use crate::message::{FlowField, Message};
use crate::presets::FLOWS;

/// Flow-step modes offered in the editor: `(label, mode byte)`.
const MODES: &[(&str, u8)] = &[("Color", 1), ("Temp", 2), ("Sleep", 7)];

/// Render the Flow tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let mut presets = row![].spacing(8);
    for (i, p) in FLOWS.iter().enumerate() {
        presets = presets.push(button(text(p.name)).on_press(Message::ApplyFlowPreset(i)));
    }
    presets = presets.push(Space::new().width(Fill));
    presets = presets.push(button(text("Stop")).on_press(Message::StopFlow));

    let mut editor = column![text("Custom flow").size(14)].spacing(6);
    if let Some(rows) = app.flow_rows.get(&d.id) {
        for (i, r) in rows.iter().enumerate() {
            let mode_name = MODES
                .iter()
                .find(|(_, m)| *m == r.mode)
                .map(|(n, _)| *n)
                .unwrap_or("Color");
            editor = editor.push(
                row![
                    text_input("dur ms", &r.duration)
                        .on_input(move |v| Message::FlowRowEdit { row: i, field: FlowField::Duration, value: v })
                        .width(80),
                    pick_list(
                        MODES.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                        Some(mode_name),
                        move |n| {
                            let m = MODES.iter().find(|(name, _)| *name == n).map(|(_, m)| *m).unwrap_or(1);
                            Message::FlowRowEdit { row: i, field: FlowField::Mode, value: m.to_string() }
                        },
                    ),
                    text_input("value", &r.value)
                        .on_input(move |v| Message::FlowRowEdit { row: i, field: FlowField::Value, value: v })
                        .width(90),
                    text_input("bright", &r.bright)
                        .on_input(move |v| Message::FlowRowEdit { row: i, field: FlowField::Bright, value: v })
                        .width(70),
                    button(text("\u{2715}")).on_press(Message::FlowRowDel(i)),
                ]
                .spacing(6)
                .align_y(iced::Center),
            );
        }
    }

    let count = app.flow_count.get(&d.id).cloned().unwrap_or_else(|| "0".into());
    editor = editor.push(
        row![
            button(text("+ Step")).on_press(Message::FlowRowAdd),
            text("Repeat (0=\u{221e}):"),
            text_input("0", &count).on_input(Message::FlowCountEdit).width(60),
            Space::new().width(Fill),
            button(text("Start")).on_press(Message::StartCustomFlow),
        ]
        .spacing(8)
        .align_y(iced::Center),
    );

    column![text("Presets").size(14), presets, editor].spacing(12).into()
}
