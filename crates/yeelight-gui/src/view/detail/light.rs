//! Light tab: a Color|Temperature segment over the matching control body. A
//! Yeelight is in exactly one mode at a time, so the two are mutually exclusive
//! here rather than separate always-on tabs.

use iced::widget::{column, row};
use iced::{Color, Element};
use yeelight_core::Device;

use super::super::components::chip;
use super::{color, enabled, white};
use crate::app::{ct_to_color, u32_to_color, App};
use crate::message::Message;

/// Whether the Light tab shows the temperature segment for this light: only temp
/// supported, or the chosen/derived segment is temp. The derived default follows
/// the device's reported mode for the main light (color_mode 2 == CT); the bg
/// light has no readback.
pub(crate) fn is_temp(app: &App, d: &Device, bg: bool) -> bool {
    let has_color = enabled(app, d, if bg { "bg_set_rgb" } else { "set_rgb" });
    let has_temp = enabled(app, d, if bg { "bg_set_ct_abx" } else { "set_ct_abx" });
    if !has_color {
        true
    } else if !has_temp {
        false
    } else {
        app.pickers
            .get(&d.id)
            .and_then(|p| if bg { p.bg_seg } else { p.main_seg })
            .unwrap_or_else(|| !bg && d.state.color_mode == Some(2))
    }
}

/// The swatch color to preview for this light: a CT-derived white when the Light
/// tab is in temperature mode, else the current/draft RGB.
pub(crate) fn preview(app: &App, d: &Device, bg: bool) -> Color {
    let p = app.pickers.get(&d.id);
    if is_temp(app, d, bg) {
        let ct = p
            .map(|p| if bg { p.bg_ct } else { p.main_ct })
            .unwrap_or_else(|| if bg { d.state.bg_ct } else { d.state.ct }.unwrap_or(4000));
        ct_to_color(ct)
    } else {
        p.map(|p| if bg { p.bg_draft } else { p.main_draft })
            .unwrap_or_else(|| {
                if bg { d.state.bg_rgb } else { d.state.rgb }
                    .map(u32_to_color)
                    .unwrap_or(Color::from_rgb(0.85, 0.8, 0.6))
            })
    }
}

/// Render the Light tab body for the given light (`bg` = background).
pub(crate) fn body<'a>(app: &'a App, d: &'a Device, bg: bool) -> Element<'a, Message> {
    let has_color = enabled(app, d, if bg { "bg_set_rgb" } else { "set_rgb" });
    let has_temp = enabled(app, d, if bg { "bg_set_ct_abx" } else { "set_ct_abx" });
    let temp = is_temp(app, d, bg);

    let inner = if temp { white::body(app, d, bg) } else { color::body(app, d, bg) };

    // Only offer the switch when the light actually supports both modes.
    if has_color && has_temp {
        let switch = row![
            chip("Color", !temp, Message::SetLightSeg { bg, temp: false }),
            chip("Temperature", temp, Message::SetLightSeg { bg, temp: true }),
        ]
        .spacing(8);
        column![switch, inner].spacing(12).into()
    } else {
        inner
    }
}
