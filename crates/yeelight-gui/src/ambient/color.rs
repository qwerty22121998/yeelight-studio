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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

impl Region {
    /// All variants, in UI display order.
    pub(crate) const ALL: [Region; 5] =
        [Region::Whole, Region::Top, Region::Bottom, Region::Left, Region::Right];
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
// ponytail: fixed 25% band; widen/narrow here if it reads wrong on ultrawides.
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

/// Reduce the cropped region of a BGRA `buf` (row pitch `stride` bytes) to one color.
pub(crate) fn extract(buf: &[u8], stride: usize, bounds: (u32, u32, u32, u32), mode: ExtractMode) -> Rgb {
    match mode {
        ExtractMode::Average => average(buf, stride, bounds),
        ExtractMode::Dominant => dominant(buf, stride, bounds),
        ExtractMode::AverageSaturated => Rgb::BLACK,   // implemented in sub-step 3
    }
}

/// Mean RGB over the cropped region. BGRA byte order: `[B, G, R, A]`.
fn average(buf: &[u8], stride: usize, (x, y, w, h): (u32, u32, u32, u32)) -> Rgb {
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for row in y..y + h {
        let base = row as usize * stride;
        for col in x..x + w {
            let i = base + col as usize * 4;
            if i + 2 >= buf.len() {
                continue;
            }
            sb += u64::from(buf[i]);
            sg += u64::from(buf[i + 1]);
            sr += u64::from(buf[i + 2]);
            n += 1;
        }
    }
    if n == 0 {
        return Rgb::BLACK;
    }
    Rgb { r: (sr / n) as u8, g: (sg / n) as u8, b: (sb / n) as u8 }
}

/// Most common color over the region, quantized to 2 bits/channel (64 bins).
/// Returns the populated bin's center color.
fn dominant(buf: &[u8], stride: usize, (x, y, w, h): (u32, u32, u32, u32)) -> Rgb {
    let mut bins = [0u32; 64];
    for row in y..y + h {
        let base = row as usize * stride;
        for col in x..x + w {
            let i = base + col as usize * 4;
            if i + 2 >= buf.len() {
                continue;
            }
            let q = |v: u8| (u16::from(v) >> 6) & 0b11; // 0..=3
            let bin = (q(buf[i + 2]) << 4) | (q(buf[i + 1]) << 2) | q(buf[i]); // r,g,b
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `w`×`h` BGRA buffer (tight stride) where every pixel is `(r,g,b)`.
    fn solid(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&[b, g, r, 255]);
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
        let got = average(&buf, 10 * 4, (0, 0, 10, 10));
        assert_eq!(got, Rgb { r: 200, g: 100, b: 50 });
    }

    #[test]
    fn average_left_half_of_split_buffer() {
        // Left 5 cols red, right 5 cols blue; averaging the left band yields red.
        let mut buf = solid(10, 4, 0, 0, 0);
        for row in 0..4u32 {
            for col in 0..10u32 {
                let i = (row as usize * 10 + col as usize) * 4;
                let (r, g, b) = if col < 5 { (255, 0, 0) } else { (0, 0, 255) };
                buf[i] = b;
                buf[i + 1] = g;
                buf[i + 2] = r;
            }
        }
        let got = average(&buf, 10 * 4, crop_bounds(Region::Left, 10, 4));
        assert_eq!(got, Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn average_respects_padded_stride() {
        // 2x2 red with 8 bytes of row padding; stride read must skip the pad.
        let mut buf = Vec::new();
        for _ in 0..2 {
            buf.extend_from_slice(&[0, 0, 255, 255, 0, 0, 255, 255]); // 2 red px
            buf.extend_from_slice(&[9, 9, 9, 9, 9, 9, 9, 9]); // padding
        }
        let got = average(&buf, 16, (0, 0, 2, 2));
        assert_eq!(got, Rgb { r: 255, g: 0, b: 0 });
    }

    #[test]
    fn to_u32_packs_rrggbb() {
        assert_eq!(Rgb { r: 0x12, g: 0x34, b: 0x56 }.to_u32(), 0x123456);
    }

    #[test]
    fn dominant_picks_majority_color() {
        // 8 of 9 px green, 1 px red → dominant bin is green-ish.
        let mut buf = solid(3, 3, 0, 200, 0);
        buf[0] = 0; buf[1] = 0; buf[2] = 255; // pixel 0 → red
        let got = extract(&buf, 3 * 4, (0, 0, 3, 3), ExtractMode::Dominant);
        assert!(got.g > 150 && got.r < 80 && got.b < 80, "got {got:?}");
    }
}
