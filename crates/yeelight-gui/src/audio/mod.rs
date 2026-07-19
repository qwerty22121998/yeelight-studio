//! Music-capture reactive light: capture audio and stream a derived color to a bulb,
//! while publishing the spectrum for a cava-style visualizer.
//!
//! The audio twin of [`crate::ambient`]. [`capture`] runs a thread publishing the
//! latest spectrum bars into a `watch`; [`run_stream`] ticks at a sink-derived rate,
//! maps the bars to one [`Rgb`] per [`dsp::MusicMode`], and pushes it to the bulb —
//! reusing [`crate::ambient::AmbientSink`] (music-mode high-fps ↔ rate-limited direct
//! ↔ CT for white bulbs). Each tick also emits the bars for the on-screen wave.

pub(crate) mod capture;
pub(crate) mod dsp;

use std::pin::Pin;
use std::time::Duration;

use iced::futures::{Stream, stream};
use tokio::sync::watch;

use crate::ambient::color::Rgb;
use crate::ambient::{AmbientSink, FlowBuf, FLOW_GAP_DIRECT, FLOW_GAP_MUSIC};
use crate::message::Message;
use dsp::{MusicMode, NUM_BARS};

/// Visualizer frame rate + capture-liveness poll. Independent of the bulb send rate so the
/// wave stays smooth even when the device can only take ~2 fps (no music mode).
const VIZ_FPS: u64 = 30;
/// Fallback direct-`set_rgb` rate — kept well under the ~144 cmd/min LAN ceiling.
const FALLBACK_FPS: u64 = 2;
/// Skip a *color send* (not the wave) if every channel moved by at most this much.
const DEDUP_DELTA: u8 = 2;
/// A beat fires when the bass energy rises past this level by at least [`BEAT_RISE`].
const BEAT_LEVEL: f32 = 0.30;
/// Minimum frame-over-frame bass jump that counts as a beat onset (for VU hue steps).
const BEAT_RISE: f32 = 0.12;
/// Hue advance per detected beat, degrees.
const BEAT_HUE_STEP: f32 = 47.0;

/// Live, user-editable music-capture settings. Input device is fixed at start (recipe
/// key); mode/targets/smooth reconfigure live. Persisted per-device in `settings.toml`;
/// `#[serde(default)]` keeps old files loadable as fields are added.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct AudioConfig {
    /// Capture device name (`None` = host default input). Changing it requires stop→start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<String>,
    /// How the spectrum collapses to the bulb's single color.
    pub(crate) mode: MusicMode,
    /// Drive the main light.
    pub(crate) main: bool,
    /// Drive the background light.
    pub(crate) bg: bool,
    /// Fade between colors over ~one send period instead of jumping.
    pub(crate) smooth: bool,
    /// Spectrum "Sensitivity" — magnitude gain before clamp. Live (Tune tab).
    pub(crate) gain: f32,
    /// "Smoothing" — per-frame bar fall factor (cava's noise_reduction). Live (Tune tab).
    pub(crate) decay: f32,
    /// Number of spectrum bars/bands. Live (Tune tab).
    pub(crate) bars: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input: None,
            mode: MusicMode::default(),
            main: true,
            bg: false,
            smooth: true,
            gain: dsp::DEFAULT_GAIN,
            decay: dsp::DEFAULT_DECAY,
            bars: dsp::NUM_BARS,
        }
    }
}

/// Build the driver stream for one device: spawn audio capture, then tick→map→send,
/// emitting [`Message::AudioSpectrum`] each tick for the visualizer and
/// [`Message::AudioError`] on a send failure. Dropping the stream stops capture.
pub(crate) fn run_stream(
    id: String,
    sink: AmbientSink,
    cfg_rx: watch::Receiver<AudioConfig>,
) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    let input = cfg_rx.borrow().input.clone();
    let (bars_tx, bars_rx) = watch::channel(vec![0.0; NUM_BARS]);

    let guard = match capture::spawn(input, bars_tx, cfg_rx.clone()) {
        Ok(g) => g,
        // No capture device could open → stop the session immediately (not a mere warning).
        Err(e) => return Box::pin(stream::once(async move { Message::AudioStopped { id, reason: e } })),
    };

    struct State {
        id: String,
        sink: AmbientSink,
        cfg_rx: watch::Receiver<AudioConfig>,
        bars_rx: watch::Receiver<Vec<f32>>,
        last_sent: Option<Rgb>,
        powered_main: bool,
        powered_bg: bool,
        errored: bool,
        beat_hue: f32,
        prev_bass: f32,
        tick: u64,
        flow: FlowBuf, // batches colors into a start_cf on the smooth path
        ended: bool,
        _guard: capture::CaptureGuard, // stops capture on drop
    }

    let state = State {
        id,
        sink,
        cfg_rx,
        bars_rx,
        last_sent: None,
        powered_main: false,
        powered_bg: false,
        errored: false,
        beat_hue: 20.0,
        prev_bass: 0.0,
        tick: 0,
        flow: FlowBuf::new(),
        ended: false,
        _guard: guard,
    };

    let driver = stream::unfold(state, |mut st| async move {
        if st.ended {
            return None; // AudioStopped already emitted — end the stream
        }
        // Fixed visualizer cadence, independent of how often we send to the bulb.
        tokio::time::sleep(Duration::from_millis(1000 / VIZ_FPS)).await;

        // Capture thread gone (device unplugged / audio server died)? Stop immediately.
        if st.bars_rx.has_changed().is_err() {
            st.ended = true;
            let id = st.id.clone();
            return Some((
                Message::AudioStopped { id, reason: "capture device unavailable".into() },
                st,
            ));
        }

        let cfg = st.cfg_rx.borrow().clone();

        // Power each enabled target on (set_rgb is a no-op on an off light) and force a send
        // when newly enabled, so ticking a target on lights it even mid-silence.
        let mut force = false;
        if cfg.main {
            if !st.powered_main {
                let _ = st.sink.client.set_power(true, yeelight_core::Effect::Sudden, None).await;
                st.powered_main = true;
                force = true;
            }
        } else {
            st.powered_main = false;
        }
        if cfg.bg {
            if !st.powered_bg {
                let _ = st.sink.client.bg_set_power(true, yeelight_core::Effect::Sudden, None).await;
                st.powered_bg = true;
                force = true;
            }
        } else {
            st.powered_bg = false;
        }

        let bars = st.bars_rx.borrow().clone();

        // Beat detection (VU hue): a bass energy onset rotates the hue.
        let bass = dsp::bands(&bars).0;
        if bass > BEAT_LEVEL && bass - st.prev_bass > BEAT_RISE {
            st.beat_hue = (st.beat_hue + BEAT_HUE_STEP).rem_euclid(360.0);
        }
        st.prev_bass = bass;

        let color = dsp::mode_color(cfg.mode, &bars, st.beat_hue);

        // Smooth = batch colors into one `start_cf` color flow (like ambient and the old
        // project) so the bulb animates the whole window. Instant = per-frame `sudden`,
        // throttled to the sink's real rate + deduped to stay under the LAN command ceiling.
        // Either way the wave (yielded below) still updates every frame. `None` = nothing sent
        // this tick (flow window still filling, or throttled/deduped).
        let mut send_err: Option<String> = None;
        let outcome: Option<Result<(), String>> = if cfg.smooth {
            let gap = if st.sink.is_music() { FLOW_GAP_MUSIC } else { FLOW_GAP_DIRECT };
            match st.flow.push(color, gap, force) {
                Some((colors, step)) => Some(st.sink.send_flow(&colors, step, cfg.main, cfg.bg).await),
                None => None,
            }
        } else {
            st.tick = st.tick.wrapping_add(1);
            let targets = u64::from(cfg.main) + u64::from(cfg.bg);
            let send_every = if st.sink.is_music() { 1 } else { (VIZ_FPS / FALLBACK_FPS) * targets.max(1) };
            let due = force || st.tick % send_every == 0;
            let should_send = due
                && (force
                    || match st.last_sent {
                        Some(prev) => color.max_delta(prev) > DEDUP_DELTA,
                        None => true,
                    });
            if should_send {
                Some(st.sink.send(color, cfg.main, cfg.bg).await)
            } else {
                None
            }
        };

        match outcome {
            None => {}
            Some(Ok(())) => {
                st.last_sent = Some(color);
                st.errored = false;
            }
            Some(Err(e)) => {
                // Music channel torn down → self-heal to direct; next frame resends.
                if st.sink.is_music() {
                    tracing::warn!("music-capture: send failed ({e}); falling back to direct");
                    st.sink.music = None;
                    st.last_sent = None;
                } else if !st.errored {
                    // Direct path: one error per failure streak, then keep going so the wave
                    // doesn't freeze and a recovered bulb resumes silently.
                    st.errored = true;
                    send_err = Some(e);
                }
            }
        }

        if let Some(error) = send_err {
            let id = st.id.clone();
            return Some((Message::AudioError { id, error }, st));
        }
        let id = st.id.clone();
        Some((Message::AudioSpectrum { id, bars, color }, st))
    });

    Box::pin(driver)
}
