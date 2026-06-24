//! Flow tab: preset flows + a custom flow editor.

use iced::widget::{button, column, pick_list, row, text, text_input, Space};
use iced::{Color, Element, Length, Length::Fill};
use yeelight_core::{Device, FlowTuple};

use super::super::components::spectrum;
use crate::app::{ct_to_color, u32_to_color, App, FlowRow};
use crate::message::{FlowField, Message};
use crate::presets::FLOWS;

/// Flow-step modes offered in the editor: `(label, mode byte)`.
const MODES: &[(&str, u8)] = &[("Color", 1), ("Temp", 2), ("Sleep", 7)];

/// Display color of a flow step (`None` = sleep step, which has no color). CT
/// steps render via the blackbody approximation, color steps from raw RGB.
fn step_color(mode: u8, value: u32) -> Option<Color> {
    match mode {
        2 => Some(ct_to_color(value as u16)),
        7 => None,
        _ => Some(u32_to_color(value)),
    }
}

/// Colors of a preset's flow tuples, sleep steps dropped.
fn preset_colors(tuples: &[FlowTuple]) -> Vec<Color> {
    tuples.iter().filter_map(|t| step_color(t.mode, t.value)).collect()
}

/// Colors of the custom-editor draft rows, sleep steps and unparsed values dropped.
fn draft_colors(rows: &[FlowRow]) -> Vec<Color> {
    rows.iter().filter_map(|r| step_color(r.mode, r.value.parse().ok()?)).collect()
}

/// Render the Flow tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let modes = super::color_modes(app, d, bg);

    // Only presets the light's color modes can run (e.g. no rgb flows on a
    // temp-only light).
    let mut presets = row![].spacing(8).align_y(iced::Top);
    for (i, p) in FLOWS.iter().enumerate() {
        if !super::fits(modes, p.needs()) {
            continue;
        }
        let colors = preset_colors(&(p.make)().0);
        let cell = column![
            button(text(p.name)).width(96).on_press(Message::ApplyFlowPreset { bg, index: i }),
            spectrum(&colors, Length::Fixed(96.0), 6.0),
        ]
        .spacing(4);
        presets = presets.push(cell);
    }
    presets = presets.push(Space::new().width(Fill));
    presets = presets.push(button(text("Stop")).on_press(Message::StopFlow { bg }));

    // Step modes the light can actually do (Sleep always; Color/Temp gated).
    let (has_rgb, has_ct) = modes;
    let step_modes: Vec<(&str, u8)> = MODES
        .iter()
        .copied()
        .filter(|(_, m)| match m {
            1 => has_rgb,
            2 => has_ct,
            _ => true,
        })
        .collect();

    let mut editor = column![text("Custom flow").size(14)].spacing(6);
    if let Some(rows) = app.flow_rows.get(&(d.id.clone(), bg)) {
        for (i, r) in rows.iter().enumerate() {
            let mode_name = MODES
                .iter()
                .find(|(_, m)| *m == r.mode)
                .map(|(n, _)| *n)
                .unwrap_or("Color");
            editor = editor.push(
                row![
                    text_input("dur ms", &r.duration)
                        .on_input(move |v| Message::FlowRowEdit { bg, row: i, field: FlowField::Duration, value: v })
                        .width(80),
                    pick_list(
                        step_modes.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                        Some(mode_name),
                        move |n| {
                            let m = MODES.iter().find(|(name, _)| *name == n).map(|(_, m)| *m).unwrap_or(1);
                            Message::FlowRowEdit { bg, row: i, field: FlowField::Mode, value: m.to_string() }
                        },
                    ),
                    text_input("value", &r.value)
                        .on_input(move |v| Message::FlowRowEdit { bg, row: i, field: FlowField::Value, value: v })
                        .width(90),
                    text_input("bright", &r.bright)
                        .on_input(move |v| Message::FlowRowEdit { bg, row: i, field: FlowField::Bright, value: v })
                        .width(70),
                    button(text("\u{2715}")).on_press(Message::FlowRowDel { bg, row: i }),
                ]
                .spacing(6)
                .align_y(iced::Center),
            );
        }
    }

    // Spectrum of the draft so far — a live preview of the flow's colors,
    // separate from the single light-tab swatch (which a flow can't represent).
    let colors = app
        .flow_rows
        .get(&(d.id.clone(), bg))
        .map(|rows| draft_colors(rows))
        .unwrap_or_default();
    if !colors.is_empty() {
        editor = editor.push(text("Preview").size(12));
        editor = editor.push(spectrum(&colors, Fill, 16.0));
    }

    let count = app.flow_count.get(&(d.id.clone(), bg)).cloned().unwrap_or_else(|| "0".into());
    editor = editor.push(
        row![
            button(text("+ Step")).on_press(Message::FlowRowAdd { bg }),
            text("Repeat (0=\u{221e}):"),
            text_input("0", &count).on_input(move |value| Message::FlowCountEdit { bg, value }).width(60),
            Space::new().width(Fill),
            button(text("Start")).on_press(Message::StartCustomFlow { bg }),
        ]
        .spacing(8)
        .align_y(iced::Center),
    );

    column![text("Presets").size(14), presets, editor].spacing(12).into()
}
