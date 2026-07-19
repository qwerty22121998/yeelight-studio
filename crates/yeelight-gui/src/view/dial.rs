//! A "filled sweep" brightness dial — the linear slider bent into a 270° arc.
//!
//! iced has no built-in circular slider, so this is a small `canvas::Program`:
//! it strokes a track, strokes the filled portion up to the current value, and
//! prints the percent in the middle. Drag anywhere around the arc (or scroll the
//! wheel while hovering) to set `1..=100`. It emits `on_change` continuously
//! while dragging and `on_release` once — mirroring the slider it replaces, so
//! `BrightDraft` updates the draft live and the command fires on release.

use iced::mouse;
use iced::widget::canvas::path::Arc;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, LineCap, Path, Stroke, Text};
use iced::{Element, Length, Point, Radians, Rectangle, Renderer, Theme};

use crate::message::Message;

/// Arc start, degrees clockwise from the +x axis — the bottom-left of the gauge.
const START: f32 = 135.0;
/// Total swept angle; the remaining 90° is the gap centred at the bottom.
const SWEEP: f32 = 270.0;
/// Widget side length in px.
const SIZE: f32 = 120.0;
/// Arc stroke width in px.
const STROKE: f32 = 12.0;

/// End angle (radians) of the filled arc for brightness `v`.
fn angle_for(v: u8) -> f32 {
    let t = (v.clamp(1, 100) as f32 - 1.0) / 99.0;
    (START + t * SWEEP).to_radians()
}

/// Brightness `1..=100` from a pointer position, given the widget bounds. Points
/// in the bottom gap clamp to the nearer end.
fn value_from(bounds: Rectangle, p: Point) -> u8 {
    let cx = bounds.x + bounds.width / 2.0;
    let cy = bounds.y + bounds.height / 2.0;
    // atan2(dy, dx) is clockwise from +x on a y-down screen — the arc's own frame.
    let phi = (p.y - cy).atan2(p.x - cx).to_degrees();
    let mut rel = (phi - START).rem_euclid(360.0);
    if rel > SWEEP {
        rel = if rel < (SWEEP + 360.0) / 2.0 { SWEEP } else { 0.0 };
    }
    (1.0 + rel / SWEEP * 99.0).round().clamp(1.0, 100.0) as u8
}

/// A circular brightness control. Build with the current value and the two
/// message-makers, then drop it into a view via `.into()`.
pub(crate) struct Dial<'a> {
    value: u8,
    on_change: Box<dyn Fn(u8) -> Message + 'a>,
    on_release: Box<dyn Fn(u8) -> Message + 'a>,
}

impl<'a> Dial<'a> {
    /// `value` is the current brightness; `on_change` fires live while dragging,
    /// `on_release` once the pointer lifts (and once per wheel tick).
    pub(crate) fn new(
        value: u8,
        on_change: impl Fn(u8) -> Message + 'a,
        on_release: impl Fn(u8) -> Message + 'a,
    ) -> Self {
        Self { value: value.clamp(1, 100), on_change: Box::new(on_change), on_release: Box::new(on_release) }
    }
}

/// Whether a drag is in progress; kept across redraws by the canvas runtime.
#[derive(Default)]
pub(crate) struct DialState {
    dragging: bool,
}

impl canvas::Program<Message> for Dial<'_> {
    type State = DialState;

    fn update(
        &self,
        state: &mut DialState,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let publish = |m: Message| canvas::Action::publish(m).and_capture();
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.position_over(bounds).is_some() =>
            {
                state.dragging = true;
                let v = value_from(bounds, cursor.position()?);
                Some(publish((self.on_change)(v)))
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                let v = value_from(bounds, cursor.position()?);
                Some(publish((self.on_change)(v)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                let v = cursor.position().map_or(self.value, |p| value_from(bounds, p));
                Some(publish((self.on_release)(v)))
            }
            // Wheel while hovering: nudge ±1 and apply immediately (no drag session).
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.position_over(bounds).is_some() =>
            {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                if dy == 0.0 {
                    return None;
                }
                let v = (self.value as i16 + if dy > 0.0 { 1 } else { -1 }).clamp(1, 100) as u8;
                Some(publish((self.on_release)(v)))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &DialState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let p = theme.extended_palette();
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        let radius = frame.width().min(frame.height()) / 2.0 - STROKE / 2.0 - 1.0;

        let arc = |end: f32| {
            Path::new(|b| {
                b.arc(Arc {
                    center,
                    radius,
                    start_angle: Radians(START.to_radians()),
                    end_angle: Radians(end),
                })
            })
        };
        let stroke = |color| Stroke::default().with_width(STROKE).with_color(color).with_line_cap(LineCap::Round);

        frame.stroke(&arc((START + SWEEP).to_radians()), stroke(p.background.strong.color));
        frame.stroke(&arc(angle_for(self.value)), stroke(p.primary.base.color));

        frame.fill_text(Text {
            content: format!("{}%", self.value),
            position: center,
            color: p.background.base.text,
            size: (radius * 0.55).into(),
            font: iced::Font::MONOSPACE,
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..Text::default()
        });

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &DialState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging || cursor.position_over(bounds).is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

impl<'a> From<Dial<'a>> for Element<'a, Message> {
    fn from(dial: Dial<'a>) -> Self {
        Canvas::new(dial).width(Length::Fixed(SIZE)).height(Length::Fixed(SIZE)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point at gauge angle `phi` (degrees) on a dial centred at the origin.
    fn at(phi: f32) -> (Rectangle, Point) {
        let bounds = Rectangle { x: -100.0, y: -100.0, width: 200.0, height: 200.0 };
        let r = 50.0;
        (bounds, Point::new(r * phi.to_radians().cos(), r * phi.to_radians().sin()))
    }
    fn val(phi: f32) -> u8 {
        let (b, p) = at(phi);
        value_from(b, p)
    }

    #[test]
    fn ends_and_middle() {
        assert_eq!(val(135.0), 1); // bottom-left  = min
        assert_eq!(val(45.0), 100); // bottom-right = max
        assert!((49..=51).contains(&val(-90.0))); // straight up ≈ 50%
    }

    #[test]
    fn bottom_gap_clamps_to_nearer_end() {
        assert_eq!(val(110.0), 1); // just left of straight-down → min
        assert_eq!(val(70.0), 100); // just right of straight-down → max
    }
}
