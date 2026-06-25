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

/// Which set-color methods each target light supports. Ambient drives a target by its
/// RGB method when available, else falls back to color temperature (a temp-only bulb).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Caps {
    /// Main light supports `set_rgb`.
    pub(crate) main_rgb: bool,
    /// Main light supports `set_ct_abx`.
    pub(crate) main_ct: bool,
    /// Background light supports `bg_set_rgb`.
    pub(crate) bg_rgb: bool,
    /// Background light supports `bg_set_ct_abx`.
    pub(crate) bg_ct: bool,
}

/// Default ambient targets for a device with the given caps: drive the main light if it
/// has any color control, otherwise the background light. Avoids defaulting `main` on for a
/// bulb that can't color its main light (which would power on a useless light and send
/// nothing while the actual color-capable bg light sits disabled).
pub(crate) fn default_targets(caps: Caps) -> (bool, bool) {
    let main = caps.main_rgb || caps.main_ct;
    let bg = caps.bg_rgb || caps.bg_ct;
    (main, bg && !main)
}

/// Where ambient colors are sent. Always has a `Client` (for power-on and the fallback
/// path); `music`, when present, makes sends fire-and-forget at the higher rate.
#[derive(Clone)]
pub(crate) struct AmbientSink {
    /// The device's request/response client.
    pub(crate) client: Arc<Client>,
    /// An active music channel, if streaming over music mode.
    pub(crate) music: Option<MusicSession>,
    /// Per-target color-method support (chooses rgb vs ct).
    pub(crate) caps: Caps,
}

impl AmbientSink {
    /// Send rate for this sink.
    fn fps(&self) -> u64 {
        if self.music.is_some() { MUSIC_FPS } else { FALLBACK_FPS }
    }

    /// Push `rgb` to the enabled targets. Each target uses its RGB method if supported,
    /// else color temperature (the screen color mapped to a warm/cool K). Over music:
    /// fire-and-forget. Direct: awaited, with a `Sudden` transition (smooth would lag).
    async fn send(&self, rgb: Rgb, main: bool, bg: bool) -> Result<(), String> {
        let v = rgb.to_u32();
        let ct = rgb_to_ct(rgb);
        if let Some(music) = &self.music {
            let mut s = music.lock().await;
            if main {
                if self.caps.main_rgb {
                    s.send("set_rgb", sudden(v)).await.map_err(|e| e.to_string())?;
                } else if self.caps.main_ct {
                    s.send("set_ct_abx", sudden(u32::from(ct))).await.map_err(|e| e.to_string())?;
                }
            }
            if bg {
                if self.caps.bg_rgb {
                    s.send("bg_set_rgb", sudden(v)).await.map_err(|e| e.to_string())?;
                } else if self.caps.bg_ct {
                    s.send("bg_set_ct_abx", sudden(u32::from(ct))).await.map_err(|e| e.to_string())?;
                }
            }
        } else {
            if main {
                if self.caps.main_rgb {
                    self.client.set_rgb(v, Effect::Sudden).await.map_err(|e| e.to_string())?;
                } else if self.caps.main_ct {
                    self.client.set_ct_abx(ct, Effect::Sudden).await.map_err(|e| e.to_string())?;
                }
            }
            if bg {
                if self.caps.bg_rgb {
                    self.client.bg_set_rgb(v, Effect::Sudden).await.map_err(|e| e.to_string())?;
                } else if self.caps.bg_ct {
                    self.client.bg_set_ct_abx(ct, Effect::Sudden).await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
}

/// `set_*`/`set_ct_abx` params with an instant transition: `[value, "sudden", 0]`.
fn sudden(value: u32) -> Vec<Value> {
    vec![json!(value), json!("sudden"), json!(0)]
}

/// Map an RGB color to an approximate correlated color temperature (`1700..=6500` K) for
/// white-only (CT) bulbs: warm/red colors → low K, cool/blue → high K.
// ponytail: linear blue-minus-red heuristic, not a real CCT curve — good enough for mood
// lighting on a temp-only bulb. Swap for McCamy's formula if accuracy ever matters.
fn rgb_to_ct(c: Rgb) -> u16 {
    const LO: f32 = 1700.0;
    const HI: f32 = 6500.0;
    let warmth = (f32::from(c.b) - f32::from(c.r)) / 255.0; // -1 (red) .. 1 (blue)
    let t = (warmth * 0.5 + 0.5).clamp(0.0, 1.0); // 0..1
    (LO + t * (HI - LO)) as u16
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
        // Whether each target is currently an active, powered-on ambient target. Reset when
        // a target is disabled, so re-enabling it re-powers and re-pushes the color.
        powered_main: bool,
        powered_bg: bool,
        errored: bool,
        _guard: capture::CaptureGuard, // stops capture on drop
    }

    let state = State {
        id,
        sink,
        cfg_rx,
        rgb_rx,
        // None: push the first captured frame unconditionally (so a genuinely dark screen
        // still drives the bulb). The Linux/grim path publishes a real frame before Ok, so
        // there's no phantom black; on the scap path the first tick may emit one black frame
        // before the first capture lands. ponytail: a 1-frame flash isn't worth gating on.
        last_sent: None,
        powered_main: false,
        powered_bg: false,
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

            // Power each enabled target on (spec: set_rgb is a no-op on an off light) and
            // force a color push when a target is newly enabled, so ticking a target on a
            // steady screen still lights it up instead of being deduped away.
            let mut force = false;
            if cfg.main {
                if !st.powered_main {
                    let _ = st.sink.client.set_power(true, Effect::Sudden, None).await;
                    st.powered_main = true;
                    force = true;
                }
            } else {
                st.powered_main = false;
            }
            if cfg.bg {
                if !st.powered_bg {
                    let _ = st.sink.client.bg_set_power(true, Effect::Sudden, None).await;
                    st.powered_bg = true;
                    force = true;
                }
            } else {
                st.powered_bg = false;
            }

            let rgb = *st.rgb_rx.borrow();
            tracing::trace!("ambient tick: read rgb={rgb:?} last_sent={:?}", st.last_sent);
            if !force
                && let Some(prev) = st.last_sent
                && rgb.max_delta(prev) <= DEDUP_DELTA
            {
                continue; // unchanged enough — skip, save quota / traffic
            }

            match st.sink.send(rgb, cfg.main, cfg.bg).await {
                Ok(()) => {
                    tracing::debug!("ambient: sent rgb={rgb:?} (main={} bg={})", cfg.main, cfg.bg);
                    st.last_sent = Some(rgb);
                    st.errored = false;
                }
                Err(e) => {
                    // A music send failing usually means the reverse channel was torn down
                    // (e.g. the user turned instant mode off in the Music tab). Self-heal to
                    // the direct client path rather than freezing; resend on the next tick.
                    if st.sink.music.is_some() {
                        tracing::warn!("ambient: music send failed ({e}); falling back to direct");
                        st.sink.music = None;
                        st.last_sent = None;
                        continue;
                    }
                    // Direct path failed: emit one error per failure streak (a dead bulb
                    // shouldn't spam the status bar every tick); keep retrying at pace.
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

#[cfg(test)]
mod tests {
    use super::*;
    use color::Rgb;

    #[test]
    fn default_targets_prefers_main_else_bg() {
        let caps = |mr, mc, br, bc| Caps { main_rgb: mr, main_ct: mc, bg_rgb: br, bg_ct: bc };
        assert_eq!(default_targets(caps(true, false, true, false)), (true, false)); // both → main
        assert_eq!(default_targets(caps(false, true, false, false)), (true, false)); // main ct-only
        assert_eq!(default_targets(caps(false, false, true, false)), (false, true)); // bg-only → bg
        assert_eq!(default_targets(caps(false, false, false, false)), (false, false)); // neither
    }

    #[test]
    fn rgb_to_ct_maps_warm_low_cool_high() {
        assert!(rgb_to_ct(Rgb { r: 255, g: 0, b: 0 }) < 2000, "red is warm");
        assert!(rgb_to_ct(Rgb { r: 0, g: 0, b: 255 }) > 6000, "blue is cool");
        let grey = rgb_to_ct(Rgb { r: 128, g: 128, b: 128 });
        assert!((3500..=4700).contains(&grey), "grey is mid: {grey}");
    }
}
