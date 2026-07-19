//! Reusable control widgets shared by the detail tabs. Pure builders; no state.

use iced::widget::{button, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Length::Fill, Theme};

use crate::message::Message;

/// A small solid color square (quick-pick / live preview).
pub(crate) fn swatch<'a>(color: Color, size: f32) -> Element<'a, Message> {
    container(Space::new().width(size).height(size))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// A pill button used for presets / quick actions. `on` highlights it.
pub(crate) fn chip<'a>(label: &'a str, on: bool, msg: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .padding(iced::Padding::from([5u16, 12]))
        .style(move |theme: &Theme, status| {
            let mut s = button::secondary(theme, status);
            if on {
                s.background = Some(Background::Color(theme.palette().primary));
                s.text_color = theme.palette().background;
            }
            s.border.radius = 14.0.into();
            s
        })
        .on_press(msg)
        .into()
}


/// A horizontal strip of equal-width color segments — a quick visual of the
/// colors a flow cycles through, instead of forcing the single preview swatch to
/// represent a multi-color sequence. Caller skips it when `colors` is empty.
pub(crate) fn spectrum<'a>(colors: &[Color], width: Length, height: f32) -> Element<'a, Message> {
    let mut strip = row![].spacing(0);
    for &c in colors {
        strip = strip.push(
            container(Space::new().width(Fill).height(height)).style(move |_: &Theme| {
                container::Style { background: Some(Background::Color(c)), ..Default::default() }
            }),
        );
    }
    container(strip)
        .width(width)
        .style(|_: &Theme| container::Style {
            border: Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// A horizontal tab strip; `selected == tab` marks the active tab.
pub(crate) fn tab_strip<'a, T: Copy + PartialEq>(
    tabs: &[(&'a str, T)],
    selected: T,
    on_select: impl Fn(T) -> Message + 'a,
) -> Element<'a, Message> {
    let mut r = row![].spacing(4);
    for (label, tab) in tabs {
        r = r.push(chip(label, *tab == selected, on_select(*tab)));
    }
    r.push(Space::new().width(Fill)).into()
}
