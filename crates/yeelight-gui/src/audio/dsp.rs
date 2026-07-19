//! Pure audio → color/spectrum logic. No I/O, no audio device — unit-tested.
//!
//! The [`Analyzer`] turns a frame of mono samples into `NUM_BARS` log-spaced
//! magnitude bars (the cava-style visualizer input); [`mode_color`] collapses
//! those bars into the single [`Rgb`] a one-color bulb can show, per [`MusicMode`].

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

use crate::ambient::color::Rgb;

/// Default number of spectrum bars (visualizer columns / analysis bands). The live
/// "Bars" knob overrides it per session; this is the fresh-config value.
pub(crate) const NUM_BARS: usize = 24;
/// FFT window size in samples (~23ms at 44.1kHz) — the analysis frame length.
pub(crate) const FFT_SIZE: usize = 1024;

/// Default per-frame bar decay ("Smoothing"): when the new magnitude is lower the bar
/// falls by this factor — the "gravity" that makes the wave read like cava (rise
/// instant, fall smooth). Higher = smoother/slower fall. Live-tunable per session.
pub(crate) const DEFAULT_DECAY: f32 = 0.80;
/// Default overall magnitude gain ("Sensitivity") before clamping to `0..=1`.
pub(crate) const DEFAULT_GAIN: f32 = 8.0;

/// How the audio drives the bulb's single color. Picked live in the UI, like the
/// ambient extraction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum MusicMode {
    /// Bass → red, mids → green, treble → blue. Loudness sets each channel.
    #[default]
    Spectrum,
    /// Overall energy pulses a warm white to the beat (single hue).
    Pulse,
    /// Spectral centroid (pitch) → hue; energy → brightness. Follows the melody.
    Rainbow,
    /// Energy → brightness; hue steps on every detected beat (party cycle).
    Vu,
}

impl MusicMode {
    /// All variants, in UI display order.
    pub(crate) const ALL: [MusicMode; 4] =
        [MusicMode::Spectrum, MusicMode::Pulse, MusicMode::Rainbow, MusicMode::Vu];
}

impl std::fmt::Display for MusicMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MusicMode::Spectrum => "Spectrum",
            MusicMode::Pulse => "Pulse",
            MusicMode::Rainbow => "Rainbow",
            MusicMode::Vu => "VU",
        })
    }
}

/// Smallest / largest bar count the analyzer accepts (keeps band ranges non-degenerate
/// and the visualizer legible). The "Bars" knob is bounded to this.
pub(crate) const BARS_RANGE: std::ops::RangeInclusive<usize> = 8..=48;

/// Streaming spectrum analyzer: window → FFT → log-binned bars with gravity decay.
/// Holds the FFT plan and reused scratch so `analyze` allocates nothing per frame.
/// Bar count, gain ("Sensitivity") and decay ("Smoothing") are live-tunable via
/// [`Analyzer::set_params`] without dropping the capture stream.
pub(crate) struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    sample_rate: f32,
    /// FFT bin index at each of the `bars.len() + 1` band edges (log-spaced).
    edges: Vec<usize>,
    bars: Vec<f32>,
    gain: f32,
    decay: f32,
}

impl Analyzer {
    /// Build an analyzer for `FFT_SIZE`-sample frames at `sample_rate` Hz with the given
    /// bar count, gain and decay. Params are clamped to sane ranges.
    pub(crate) fn new(sample_rate: f32, num_bars: usize, gain: f32, decay: f32) -> Self {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        // Hann window — reduces spectral leakage so bars are crisp.
        let window = (0..FFT_SIZE)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / FFT_SIZE as f32).cos())
            .collect();
        let num_bars = clamp_bars(num_bars);
        Self {
            fft,
            buf: vec![Complex::default(); FFT_SIZE],
            scratch,
            window,
            sample_rate,
            edges: band_edges(sample_rate, num_bars),
            bars: vec![0.0; num_bars],
            gain: gain.max(0.0),
            decay: decay.clamp(0.0, 0.99),
        }
    }

    /// Apply the live "Tune" params. Gain/decay take effect next frame; changing the bar
    /// count rebuilds the log band edges and resets the bars (only work when it changes).
    pub(crate) fn set_params(&mut self, num_bars: usize, gain: f32, decay: f32) {
        self.gain = gain.max(0.0);
        self.decay = decay.clamp(0.0, 0.99);
        let num_bars = clamp_bars(num_bars);
        if num_bars != self.bars.len() {
            self.edges = band_edges(self.sample_rate, num_bars);
            self.bars = vec![0.0; num_bars];
        }
    }

    /// Fold a mono `frame` of exactly `FFT_SIZE` samples into the current bars.
    /// Frames shorter than `FFT_SIZE` are ignored (the caller buffers up to size).
    pub(crate) fn analyze(&mut self, frame: &[f32]) {
        if frame.len() < FFT_SIZE {
            return;
        }
        let frame = &frame[frame.len() - FFT_SIZE..];
        for (i, c) in self.buf.iter_mut().enumerate() {
            *c = Complex { re: frame[i] * self.window[i], im: 0.0 };
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);

        for b in 0..self.bars.len() {
            let (lo, hi) = (self.edges[b], (self.edges[b + 1]).max(self.edges[b] + 1));
            let mut sum = 0.0;
            let mut n = 0u32;
            for k in lo..hi {
                sum += self.buf[k].norm();
                n += 1;
            }
            let raw = if n > 0 { sum / n as f32 } else { 0.0 };
            // Perceptual-ish compression: sqrt of energy, gained and clamped.
            let norm = (raw / FFT_SIZE as f32 * self.gain).sqrt().clamp(0.0, 1.0);
            self.bars[b] = if norm > self.bars[b] { norm } else { (self.bars[b] * self.decay).max(norm) };
        }
    }

    /// The current bars (`0..=1` each), newest analysis, for the visualizer.
    pub(crate) fn bars(&self) -> &[f32] {
        &self.bars
    }
}

/// Clamp a requested bar count into [`BARS_RANGE`] (guards degenerate bands / a corrupt
/// persisted config from panicking [`bands`]).
fn clamp_bars(n: usize) -> usize {
    n.clamp(*BARS_RANGE.start(), *BARS_RANGE.end())
}

/// Log-spaced FFT-bin edges spanning `40 Hz .. min(16 kHz, Nyquist)` across `num_bars`
/// bands. Low bands may collapse to a single bin at coarse resolution;
/// [`Analyzer::analyze`] widens any empty band to one bin.
fn band_edges(sample_rate: f32, num_bars: usize) -> Vec<usize> {
    let f_lo = 40.0_f32;
    let f_hi = (sample_rate / 2.0).min(16_000.0).max(f_lo * 2.0);
    let bin_hz = sample_rate / FFT_SIZE as f32;
    let max_bin = FFT_SIZE / 2;
    (0..=num_bars)
        .map(|b| {
            let t = b as f32 / num_bars as f32;
            let f = f_lo * (f_hi / f_lo).powf(t);
            ((f / bin_hz).round() as usize).clamp(0, max_bin)
        })
        .collect()
}

/// Mean of the bars in `[lo, hi)`.
fn mean(bars: &[f32], lo: usize, hi: usize) -> f32 {
    let s: f32 = bars[lo..hi].iter().sum();
    s / (hi - lo).max(1) as f32
}

/// Bass / mid / treble energies from the bars (low quarter / middle half / top quarter).
/// Works for any bar count `>= 4` (guaranteed by [`BARS_RANGE`]).
pub(crate) fn bands(bars: &[f32]) -> (f32, f32, f32) {
    let n = bars.len();
    let q = (n / 4).max(1);
    (mean(bars, 0, q), mean(bars, q, n - q), mean(bars, n - q, n))
}

/// Overall perceived loudness `0..=1`, weighted toward bass (which dominates music energy).
pub(crate) fn energy(bars: &[f32]) -> f32 {
    let (bass, mid, treble) = bands(bars);
    ((bass * 1.2 + mid + treble * 0.8) / 2.2).clamp(0.0, 1.0)
}

/// Spectral centroid as a fraction `0..=1` of the band range (0 = all bass, 1 = all treble).
/// Falls back to `0.5` when there's essentially no signal.
pub(crate) fn centroid(bars: &[f32]) -> f32 {
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for (i, &v) in bars.iter().enumerate() {
        num += i as f32 * v;
        den += v;
    }
    if den < 1e-3 { 0.5 } else { num / den / (bars.len().max(2) - 1) as f32 }
}

/// Collapse the spectrum bars into the one color a single bulb shows, per `mode`.
/// `beat_hue` is the caller-maintained rotating hue used only by [`MusicMode::Vu`].
pub(crate) fn mode_color(mode: MusicMode, bars: &[f32], beat_hue: f32) -> Rgb {
    let (bass, mid, treble) = bands(bars);
    let e = energy(bars);
    match mode {
        MusicMode::Spectrum => rgb(bass * 255.0, mid * 255.0, treble * 255.0),
        // Warm white scaled by energy (loudness → brightness, baked into the RGB).
        MusicMode::Pulse => {
            let e = e.powf(0.8);
            rgb(255.0 * e, 150.0 * e, 64.0 * e)
        }
        MusicMode::Rainbow => hsv(centroid(bars) * 300.0, 0.92, e.max(0.05)),
        MusicMode::Vu => hsv(beat_hue, 0.85, e.max(0.04)),
    }
}

/// Decorative bars for the idle wave (capture stopped): two slow travelling sines make a
/// soft rolling ripple so the visualizer stays alive, like the preview. `phase` is a frame
/// counter; the result is `n` gentle values in roughly `0.03..=0.35` (matches the
/// configured bar count so the idle and live waves share a width).
pub(crate) fn idle_bars(phase: u32, n: usize) -> Vec<f32> {
    let t = phase as f32 * 0.08;
    (0..n)
        .map(|i| {
            let x = i as f32;
            let a = (t + x * 0.45).sin();
            let b = (t * 0.7 - x * 0.28).sin();
            (0.19 + 0.09 * (a * 0.6 + b * 0.4)).clamp(0.0, 1.0)
        })
        .collect()
}

/// Clamp three `0..=255`-ish floats into an [`Rgb`].
fn rgb(r: f32, g: f32, b: f32) -> Rgb {
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Rgb { r: c(r), g: c(g), b: c(b) }
}

/// HSV → [`Rgb`]. `h` in degrees (wrapped), `s`/`v` in `0..=1`.
pub(crate) fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    rgb((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_mode_maps_bands_to_channels() {
        let mut bass_only = [0.0; NUM_BARS];
        bass_only[0] = 1.0;
        bass_only[1] = 1.0;
        let c = mode_color(MusicMode::Spectrum, &bass_only, 0.0);
        assert!(c.r > c.g && c.r > c.b, "bass → red: {c:?}");

        let mut treble_only = [0.0; NUM_BARS];
        treble_only[NUM_BARS - 1] = 1.0;
        let c = mode_color(MusicMode::Spectrum, &treble_only, 0.0);
        assert!(c.b > c.r, "treble → blue: {c:?}");
    }

    #[test]
    fn pulse_scales_with_energy() {
        let quiet = mode_color(MusicMode::Pulse, &[0.02; NUM_BARS], 0.0);
        let loud = mode_color(MusicMode::Pulse, &[1.0; NUM_BARS], 0.0);
        assert!(loud.r > quiet.r && loud.g > quiet.g, "louder is brighter: {quiet:?} {loud:?}");
        assert!(loud.r > loud.g && loud.g > loud.b, "warm white: {loud:?}");
    }

    #[test]
    fn centroid_tracks_where_the_energy_is() {
        let mut low = [0.0; NUM_BARS];
        low[0] = 1.0;
        let mut high = [0.0; NUM_BARS];
        high[NUM_BARS - 1] = 1.0;
        assert!(centroid(&low) < 0.15, "bass-heavy → low centroid");
        assert!(centroid(&high) > 0.85, "treble-heavy → high centroid");
        assert!((centroid(&[0.0; NUM_BARS]) - 0.5).abs() < 1e-6, "silence → mid");
    }

    #[test]
    fn idle_bars_stay_in_gentle_range_and_move() {
        let a = idle_bars(0, NUM_BARS);
        let b = idle_bars(20, NUM_BARS);
        assert_eq!(a.len(), NUM_BARS, "idle wave matches the configured bar count");
        assert!(a.iter().all(|&v| (0.0..=0.5).contains(&v)), "idle bars stay gentle");
        assert!(a != b, "idle wave animates with phase");
    }

    #[test]
    fn analyzer_respects_configured_bar_count() {
        let mut a = Analyzer::new(44_100.0, 12, DEFAULT_GAIN, DEFAULT_DECAY);
        assert_eq!(a.bars().len(), 12, "honors the requested bar count");
        a.set_params(32, DEFAULT_GAIN, DEFAULT_DECAY);
        assert_eq!(a.bars().len(), 32, "reconfigures the bar count live");
        // Out-of-range requests clamp instead of panicking on a corrupt config.
        a.set_params(1, DEFAULT_GAIN, DEFAULT_DECAY);
        assert_eq!(a.bars().len(), *BARS_RANGE.start());
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(hsv(120.0, 1.0, 1.0), Rgb { r: 0, g: 255, b: 0 });
        assert_eq!(hsv(240.0, 1.0, 1.0), Rgb { r: 0, g: 0, b: 255 });
        assert_eq!(hsv(0.0, 0.0, 0.0), Rgb { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn analyzer_puts_a_tone_in_the_right_band() {
        // A 3kHz sine should light a treble band far above the bass bands.
        let sr = 44_100.0;
        let f = 3_000.0;
        let frame: Vec<f32> = (0..FFT_SIZE)
            .map(|n| (std::f32::consts::TAU * f * n as f32 / sr).sin())
            .collect();
        let mut a = Analyzer::new(sr, NUM_BARS, DEFAULT_GAIN, DEFAULT_DECAY);
        a.analyze(&frame);
        let bars = a.bars();
        let (bass, _mid, treble) = bands(bars);
        assert!(treble > bass, "3kHz tone: treble {treble} should exceed bass {bass}");
        // The single loudest bar's band should actually contain 3kHz.
        let peak = bars.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let edges = band_edges(sr, NUM_BARS);
        let bin_hz = sr / FFT_SIZE as f32;
        let (lo, hi) = (edges[peak] as f32 * bin_hz, edges[peak + 1] as f32 * bin_hz);
        assert!((lo..=hi + bin_hz).contains(&f), "peak band {lo}..{hi} Hz should hold {f}");
    }
}
