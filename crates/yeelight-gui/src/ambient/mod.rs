//! Ambient screen-capture light: capture a screen region and stream its color to a bulb.
//!
//! `capture` runs a thread publishing the latest region color into a `watch`; [`run_stream`]
//! ticks at a sink-derived rate, dedups, and pushes the color to the bulb's main/bg lights.

pub(crate) mod capture;
pub(crate) mod color;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use iced::futures::{stream, Stream};
use serde_json::{json, Value};
use tokio::sync::watch;
use yeelight_core::{Client, Effect};

use crate::message::{Message, MusicSession};
use color::Rgb;

/// Music-mode send rate (no device quota).
const MUSIC_FPS: u64 = 15;
/// Fallback `set_rgb` send rate — kept well under the ~144 cmd/min LAN ceiling.
const FALLBACK_FPS: u64 = 2;
/// Skip a send if every channel moved by at most this much since the last send.
const DEDUP_DELTA: u8 = 4;

/// Live, user-editable ambient settings (region/mode/targets). Monitor is fixed at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbientConfig {
    /// Which screen slice feeds the bulb.
    pub(crate) region: color::Region,
    /// How pixels collapse to one color.
    pub(crate) mode: color::ExtractMode,
    /// Display id captured (None = primary). Changing it requires stop→start.
    pub(crate) monitor_id: Option<u32>,
    /// Drive the main light (only settable if `set_rgb` is supported).
    pub(crate) main: bool,
    /// Drive the background light (only settable if `bg_set_rgb` is supported).
    pub(crate) bg: bool,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self {
            region: color::Region::default(),
            mode: color::ExtractMode::default(),
            monitor_id: None,
            main: true,
            bg: false,
        }
    }
}

/// Where ambient colors are sent. Always has a `Client` (for power-on and the fallback
/// path); `music`, when present, makes sends fire-and-forget at the higher rate.
#[derive(Clone)]
pub(crate) struct AmbientSink {
    /// The device's request/response client.
    pub(crate) client: Arc<Client>,
    /// An active music channel, if streaming over music mode.
    pub(crate) music: Option<MusicSession>,
}

impl AmbientSink {
    /// Send rate for this sink.
    fn fps(&self) -> u64 {
        if self.music.is_some() { MUSIC_FPS } else { FALLBACK_FPS }
    }

    /// Push `rgb` to the enabled targets. Over music: fire-and-forget. Direct: awaited
    /// `set_rgb`/`bg_set_rgb` with a `Sudden` transition (smooth would queue and lag).
    async fn send(&self, rgb: Rgb, main: bool, bg: bool) -> Result<(), String> {
        let v = rgb.to_u32();
        if let Some(music) = &self.music {
            let mut s = music.lock().await;
            if main {
                s.send("set_rgb", sudden(v)).await.map_err(|e| e.to_string())?;
            }
            if bg {
                s.send("bg_set_rgb", sudden(v)).await.map_err(|e| e.to_string())?;
            }
        } else {
            if main {
                self.client.set_rgb(v, Effect::Sudden).await.map_err(|e| e.to_string())?;
            }
            if bg {
                self.client.bg_set_rgb(v, Effect::Sudden).await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

/// `set_rgb` params with an instant transition.
fn sudden(rgb: u32) -> Vec<Value> {
    vec![json!(rgb), json!("sudden"), json!(0)]
}

/// Build the driver stream for one device: spawn capture, then tick→dedup→send.
/// Yields a [`Message::AmbientError`] only when a send fails (otherwise silent).
/// Dropping the stream (subscription removed) stops capture via the guard.
pub(crate) fn run_stream(
    id: String,
    sink: AmbientSink,
    cfg_rx: watch::Receiver<AmbientConfig>,
) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    let monitor_id = cfg_rx.borrow().monitor_id;
    let (rgb_tx, rgb_rx) = watch::channel(Rgb::BLACK);

    let guard = match capture::spawn(monitor_id, cfg_rx.clone(), rgb_tx) {
        Ok(g) => g,
        Err(e) => {
            // One error message, then the stream ends; the UI shows it and stays "on"
            // until the user toggles off.
            return Box::pin(stream::once(async move { Message::AmbientError { id, error: e } }));
        }
    };

    struct State {
        id: String,
        sink: AmbientSink,
        cfg_rx: watch::Receiver<AmbientConfig>,
        rgb_rx: watch::Receiver<Rgb>,
        last_sent: Option<Rgb>,
        powered_on: bool,
        errored: bool,
        _guard: capture::CaptureGuard, // stops capture on drop
    }

    let state = State {
        id,
        sink,
        cfg_rx,
        rgb_rx,
        // Seed with black so the initial (pre-first-frame) black is deduped, not flashed.
        last_sent: Some(Rgb::BLACK),
        powered_on: false,
        errored: false,
        _guard: guard,
    };

    let driver = stream::unfold(state, |mut st| async move {
        loop {
            let cfg = st.cfg_rx.borrow().clone();

            // Pace the loop. Music has no quota. The direct path issues one command per
            // enabled target per tick, so stretch the period by the target count to keep
            // the total under the device's LAN command ceiling (2 fps × 2 targets would
            // otherwise be 240 cmd/min, over ~144).
            let base = Duration::from_millis(1000 / st.sink.fps());
            let period = if st.sink.music.is_some() {
                base
            } else {
                base * (u32::from(cfg.main) + u32::from(cfg.bg)).max(1)
            };
            tokio::time::sleep(period).await;

            // Power the enabled target(s) on once so set_rgb isn't a no-op (spec: only when on).
            if !st.powered_on {
                if cfg.main {
                    let _ = st.sink.client.set_power(true, Effect::Sudden, None).await;
                }
                if cfg.bg {
                    let _ = st.sink.client.bg_set_power(true, Effect::Sudden, None).await;
                }
                st.powered_on = true;
            }

            let rgb = *st.rgb_rx.borrow();
            if let Some(prev) = st.last_sent
                && rgb.max_delta(prev) <= DEDUP_DELTA
            {
                continue; // unchanged enough — skip, save quota / traffic
            }

            match st.sink.send(rgb, cfg.main, cfg.bg).await {
                Ok(()) => {
                    st.last_sent = Some(rgb);
                    st.errored = false;
                }
                Err(e) => {
                    // Emit one error per failure streak (a dead bulb shouldn't spam the
                    // status bar every tick); keep retrying at the loop's pace.
                    if !st.errored {
                        st.errored = true;
                        let id = st.id.clone();
                        return Some((Message::AmbientError { id, error: e }, st));
                    }
                }
            }
        }
    });

    Box::pin(driver)
}
