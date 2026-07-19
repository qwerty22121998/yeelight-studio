//! A cava-style spectrum wave — a `canvas::Program` that draws the music-capture
//! bars as vertical columns, colored bass→red / mid→green / treble→blue across the
//! frequency axis to echo the light. Purely presentational: no interaction, redrawn
//! each [`crate::message::Message::AudioSpectrum`] tick (~30 fps).

use iced::widget::canvas::{self, Canvas, Frame, Geometry};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};

use crate::message::Message;

/// Fixed wave height in px.
const HEIGHT: f32 = 132.0;
/// Gap between bars in px.
const GAP: f32 = 3.0;

/// Frequency-axis anchor colors (theme-native Gruvbox hues): bass, mid, treble.
const BASS: Color = Color::from_rgb(0.917, 0.412, 0.384); // #ea6962
const MID: Color = Color::from_rgb(0.663, 0.714, 0.396); // #a9b665
const TREBLE: Color = Color::from_rgb(0.490, 0.682, 0.639); // #7daea3

/// A spectrum wave over `bars` (each `0..=1`) — either the live capture frame or the
/// generated idle ripple. Owns the small `Vec` so callers can pass computed bars.
pub(crate) struct Wave {
    bars: Vec<f32>,
}

impl Wave {
    /// Build a wave for the given bars (empty renders nothing).
    pub(crate) fn new(bars: Vec<f32>) -> Self {
        Self { bars }
    }
}

/// Color for bar `i` of `n`: lerp bass→mid over the low half, mid→treble over the high.
fn bar_color(i: usize, n: usize) -> Color {
    let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
    let lerp = |a: Color, b: Color, u: f32| Color {
        r: a.r + (b.r - a.r) * u,
        g: a.g + (b.g - a.g) * u,
        b: a.b + (b.b - a.b) * u,
        a: 1.0,
    };
    if t < 0.5 { lerp(BASS, MID, t * 2.0) } else { lerp(MID, TREBLE, (t - 0.5) * 2.0) }
}

impl canvas::Program<Message> for Wave {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let n = self.bars.len();
        if n == 0 || w <= 0.0 {
            return vec![frame.into_geometry()];
        }
        let track = {
            let c = theme.extended_palette().background.strong.color;
            Color { a: 0.22, ..c }
        };
        let bw = ((w - GAP * (n as f32 - 1.0)) / n as f32).max(1.0);
        for (i, &v) in self.bars.iter().enumerate() {
            let x = i as f32 * (bw + GAP);
            let bh = (v.clamp(0.0, 1.0) * (h - 2.0)).max(2.0);
            frame.fill_rectangle(Point::new(x, 0.0), Size::new(bw, h), track);
            frame.fill_rectangle(Point::new(x, h - bh), Size::new(bw, bh), bar_color(i, n));
        }
        vec![frame.into_geometry()]
    }
}

impl<'a> From<Wave> for Element<'a, Message> {
    fn from(wave: Wave) -> Self {
        Canvas::new(wave).width(Length::Fill).height(Length::Fixed(HEIGHT)).into()
    }
}
