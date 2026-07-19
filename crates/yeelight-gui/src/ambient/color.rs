//! Pure color extraction from a captured frame. No I/O, no platform code — unit-tested.

/// An 8-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Rgb {
    /// Red.
    pub(crate) r: u8,
    /// Green.
    pub(crate) g: u8,
    /// Blue.
    pub(crate) b: u8,
}

impl Rgb {
    /// Black.
    pub(crate) const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    /// Pack into `0xRRGGBB` for the device protocol.
    pub(crate) fn to_u32(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }

    /// Max per-channel absolute difference to `other` (for send-dedup).
    pub(crate) fn max_delta(self, other: Rgb) -> u8 {
        let d = |a: u8, b: u8| a.abs_diff(b);
        d(self.r, other.r).max(d(self.g, other.g)).max(d(self.b, other.b))
    }
}

/// Which slice of the screen feeds the bulb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum Region {
    /// The whole captured frame.
    #[default]
    Whole,
    /// Top band, full width.
    Top,
    /// Bottom band, full width.
    Bottom,
    /// Left band, full height.
    Left,
    /// Right band, full height.
    Right,
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Region::Whole => "Whole screen",
            Region::Top => "Top",
            Region::Bottom => "Bottom",
            Region::Left => "Left",
            Region::Right => "Right",
        })
    }
}

/// How the region's pixels collapse to one color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum ExtractMode {
    /// Mean of all pixels.
    #[default]
    Average,
    /// Most common quantized color.
    Dominant,
    /// Mean, then saturation boosted.
    AverageSaturated,
}

impl ExtractMode {
    /// All variants, in UI display order.
    pub(crate) const ALL: [ExtractMode; 3] =
        [ExtractMode::Average, ExtractMode::Dominant, ExtractMode::AverageSaturated];
}

impl std::fmt::Display for ExtractMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ExtractMode::Average => "Average",
            ExtractMode::Dominant => "Dominant",
            ExtractMode::AverageSaturated => "Average + saturation",
        })
    }
}

/// Edge-band depth as a fraction of the perpendicular dimension.
pub(crate) const EDGE_FRACTION: f32 = 0.25;

/// The `(x, y, w, h)` sub-rectangle of a `w`×`h` frame selected by `region`.
/// Edge bands are `EDGE_FRACTION` of the perpendicular dimension (min 1px).
pub(crate) fn crop_bounds(region: Region, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let band = |dim: u32| ((dim as f32 * EDGE_FRACTION) as u32).max(1).min(dim);
    match region {
        Region::Whole => (0, 0, w, h),
        Region::Top => (0, 0, w, band(h)),
        Region::Bottom => (0, h - band(h), w, band(h)),
        Region::Left => (0, 0, band(w), h),
        Region::Right => (w - band(w), 0, band(w), h),
    }
}

/// Reduce the cropped region of a packed-RGBA `buf` (row pitch `stride` bytes, 4 bytes/px
/// in `R, G, B, A` order) to one color. Both capture backends deliver RGBA: `xcap`'s
/// `RgbaImage` and `libwayshot`'s `DynamicImage::to_rgba8()`.
pub(crate) fn extract_rgba(buf: &[u8], stride: usize, bounds: (u32, u32, u32, u32), mode: ExtractMode) -> Rgb {
    match mode {
        ExtractMode::Average => average_rgba(buf, stride, bounds),
        ExtractMode::Dominant => dominant_rgba(buf, stride, bounds),
        ExtractMode::AverageSaturated => saturate(average_rgba(buf, stride, bounds), 1.4),
    }
}

/// Mean RGB over the cropped region. RGBA byte order: `[R, G, B, A]`; alpha ignored.
fn average_rgba(buf: &[u8], stride: usize, (x, y, w, h): (u32, u32, u32, u32)) -> Rgb {
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for row in y..y + h {
        let base = row as usize * stride;
        for col in x..x + w {
            let i = base + col as usize * 4;
            if i + 2 >= buf.len() {
                continue;
            }
            sr += u64::from(buf[i]);
            sg += u64::from(buf[i + 1]);
            sb += u64::from(buf[i + 2]);
            n += 1;
        }
    }
    if n == 0 {
        return Rgb::BLACK;
    }
    Rgb { r: (sr / n) as u8, g: (sg / n) as u8, b: (sb / n) as u8 }
}

/// Most common color over the region, quantized to 2 bits/channel (64 bins). RGBA order.
/// Returns the populated bin's center color.
fn dominant_rgba(buf: &[u8], stride: usize, (x, y, w, h): (u32, u32, u32, u32)) -> Rgb {
    let mut bins = [0u32; 64];
    for row in y..y + h {
        let base = row as usize * stride;
        for col in x..x + w {
            let i = base + col as usize * 4;
            if i + 2 >= buf.len() {
                continue;
            }
            let q = |v: u8| (u16::from(v) >> 6) & 0b11; // 0..=3
            let bin = (q(buf[i]) << 4) | (q(buf[i + 1]) << 2) | q(buf[i + 2]); // r,g,b
            bins[bin as usize] += 1;
        }
    }
    let best = bins.iter().enumerate().max_by_key(|&(_, c)| *c).map_or(0, |(i, _)| i) as u8;
    let center = |level: u8| level.saturating_mul(64).saturating_add(32); // bin center
    Rgb {
        r: center((best >> 4) & 0b11),
        g: center((best >> 2) & 0b11),
        b: center(best & 0b11),
    }
}

/// Scale a color's saturation by `factor` (clamped to valid RGB). Operates in HSV:
/// keeps hue and value, multiplies saturation. A neutral (grey) color is unchanged.
fn saturate(c: Rgb, factor: f32) -> Rgb {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max == min {
        return c; // grey — no hue to saturate
    }
    let v = f32::from(max);
    let s = (f32::from(max - min) / v * factor).clamp(0.0, 1.0); // new saturation
    // Rebuild each channel toward `v` keeping its relative position between min..max.
    let lift = |ch: u8| {
        let frac = f32::from(ch - min) / f32::from(max - min); // 0..1 within the spread
        let lo = v * (1.0 - s);
        (lo + frac * (v - lo)).round().clamp(0.0, 255.0) as u8
    };
    Rgb { r: lift(c.r), g: lift(c.g), b: lift(c.b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `w`×`h` RGBA buffer (tight stride) where every pixel is `(r,g,b)`.
    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&[r, g, b, 255]);
        }
        v
    }

    #[test]
    fn crop_bounds_regions() {
        assert_eq!(crop_bounds(Region::Whole, 100, 80), (0, 0, 100, 80));
        assert_eq!(crop_bounds(Region::Top, 100, 80), (0, 0, 100, 20));
        assert_eq!(crop_bounds(Region::Bottom, 100, 80), (0, 60, 100, 20));
        assert_eq!(crop_bounds(Region::Left, 100, 80), (0, 0, 25, 80));
        assert_eq!(crop_bounds(Region::Right, 100, 80), (75, 0, 25, 80));
    }

    #[test]
    fn average_solid_buffer() {
        let buf = solid(10, 10, 200, 100, 50);
        let got = average_rgba(&buf, 10 * 4, (0, 0, 10, 10));
        assert_eq!(got, Rgb { r: 200, g: 100, b: 50 });
    }

    #[test]
    fn average_left_band_of_split_buffer() {
        // Left 5 cols red, right 5 cols blue; averaging the Left band yields red.
        let (w, h) = (10u32, 4u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for row in 0..h {
            for col in 0..w {
                let i = (row * w + col) as usize * 4;
                let (r, g, b) = if col < 5 { (255, 0, 0) } else { (0, 0, 255) };
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
            }
        }
        let got = extract_rgba(&buf, (w * 4) as usize, crop_bounds(Region::Left, w, h), ExtractMode::Average);
        assert_eq!(got, Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn average_respects_padded_stride() {
        // 2x2 red with 8 bytes of row padding; stride read must skip the pad.
        let mut buf = Vec::new();
        for _ in 0..2 {
            buf.extend_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255]); // 2 red px
            buf.extend_from_slice(&[9, 9, 9, 9, 9, 9, 9, 9]); // padding
        }
        let got = average_rgba(&buf, 16, (0, 0, 2, 2));
        assert_eq!(got, Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn to_u32_packs_rrggbb() {
        assert_eq!(Rgb { r: 0x12, g: 0x34, b: 0x56 }.to_u32(), 0x123456);
    }

    #[test]
    fn saturated_is_more_saturated_than_plain_average() {
        // A muted color: average stays muted, AverageSaturated pushes saturation up.
        let buf = solid(8, 8, 150, 120, 100);
        let avg = extract_rgba(&buf, 8 * 4, (0, 0, 8, 8), ExtractMode::Average);
        let sat = extract_rgba(&buf, 8 * 4, (0, 0, 8, 8), ExtractMode::AverageSaturated);
        let spread = |c: Rgb| c.r.max(c.g).max(c.b) - c.r.min(c.g).min(c.b);
        assert!(spread(sat) > spread(avg), "avg {avg:?} sat {sat:?}");
    }

    #[test]
    fn saturated_keeps_grey_grey() {
        // Zero-saturation input has nothing to boost; stays neutral.
        let buf = solid(4, 4, 128, 128, 128);
        let sat = extract_rgba(&buf, 4 * 4, (0, 0, 4, 4), ExtractMode::AverageSaturated);
        assert_eq!(sat, Rgb { r: 128, g: 128, b: 128 });
    }

    #[test]
    fn dominant_picks_majority_color() {
        // 8 of 9 px green, 1 px red → dominant bin is green-ish.
        let mut buf = solid(3, 3, 0, 200, 0);
        buf[0] = 255; buf[1] = 0; buf[2] = 0; // pixel 0 → red
        let got = extract_rgba(&buf, 3 * 4, (0, 0, 3, 3), ExtractMode::Dominant);
        assert!(got.g > 150 && got.r < 80 && got.b < 80, "got {got:?}");
    }
}
