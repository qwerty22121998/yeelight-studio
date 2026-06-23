//! Color tab: an iced_aw color-picker overlay plus quick-pick swatches.

use iced::widget::{button, column, row, text};
use iced::{Color, Element};
use iced_aw::ColorPicker;
use yeelight_core::Device;

use super::super::components::swatch;
use crate::app::{u32_to_color, App};
use crate::message::{Btn, Message};

/// One-tap common colors shown under the picker.
const QUICK: &[u32] = &[0xFF0000, 0xFF7F00, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x0000FF, 0xFF00FF, 0xFFFFFF];

/// Render the Color tab body.
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let bg = app.target_light().is_bg();
    let ps = app.pickers.get(&d.id);
    let open = ps.map(|p| if bg { p.bg_open } else { p.main_open }).unwrap_or(false);
    let draft = ps
        .map(|p| if bg { p.bg_draft } else { p.main_draft })
        .unwrap_or_else(|| d.state.rgb.map(u32_to_color).unwrap_or(Color::WHITE));

    let press = (!app.btn_busy(&d.id, bg, Btn::Color)).then_some(Message::OpenPicker { bg });
    let picker = ColorPicker::new(
        open,
        draft,
        button(text("Change color")).on_press_maybe(press),
        Message::CancelPicker { bg },
        move |color| Message::PickColor { bg, color },
    );

    let mut quick = row![].spacing(8);
    for &rgb in QUICK {
        quick = quick.push(
            button(swatch(u32_to_color(rgb), 26.0))
                .style(button::text)
                .on_press(Message::PickColor { bg, color: u32_to_color(rgb) }),
        );
    }

    column![picker, text("Quick colors").size(13), quick].spacing(12).into()
}
