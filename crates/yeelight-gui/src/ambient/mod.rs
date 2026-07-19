//! Ambient screen-capture light: capture a screen region and stream its color to a bulb.
//!
//! `capture` runs a thread publishing the latest region color into a `watch`; [`run_stream`]
//! ticks at a sink-derived rate, dedups, and pushes the color to the bulb's main/bg lights.

pub(crate) mod capture;
pub(crate) mod color;

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::{stream, Stream};
use serde_json::json;
use tokio::sync::watch;
use yeelight_core::{Client, Effect, FlowAction, FlowExpr, FlowTuple};

use crate::message::{Message, MusicSession};
use color::Rgb;

/// Music-mode send rate (no device quota).
const MUSIC_FPS: u64 = 15;
/// Fallback `set_rgb` send rate — kept well under the ~144 cmd/min LAN ceiling.
const FALLBACK_FPS: u64 = 2;
/// Skip a send if every channel moved by at most this much since the last send.
const DEDUP_DELTA: u8 = 4;

/// Most colors packed into one smooth-transition color flow (older colors drop off). A
/// short window keeps each `start_cf` small while still animating the whole span.
const MAX_FLOW_STEPS: usize = 9;
/// How long the smooth (color-flow) path accumulates colors before flushing one flow.
/// Music has no command quota so it flushes often; the direct path flushes slower to stay
/// under the ~144 cmd/min LAN ceiling (≤2 targets × 1/s = 120/min).
pub(crate) const FLOW_GAP_MUSIC: Duration = Duration::from_millis(500);
/// Direct-path flow flush window (see [`FLOW_GAP_MUSIC`]).
pub(crate) const FLOW_GAP_DIRECT: Duration = Duration::from_millis(1000);

/// Live, user-editable ambient settings (region/mode/targets). Monitor is fixed at start.
/// Persisted per-device in `settings.toml`; `#[serde(default)]` keeps old files loadable
/// as new fields are added.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct AmbientConfig {
    /// Which screen slice feeds the bulb.
    pub(crate) region: color::Region,
    /// How pixels collapse to one color.
    pub(crate) mode: color::ExtractMode,
    /// Display id captured (None = primary). Changing it requires stop→start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) monitor_id: Option<u32>,
    /// Drive the main light (settable if it has any color control — `set_rgb` or, for a
    /// white-only bulb, `set_ct_abx`).
    pub(crate) main: bool,
    /// Drive the background light (settable if it has any color control — `bg_set_rgb` or,
    /// for a white-only bulb, `bg_set_ct_abx`).
    pub(crate) bg: bool,
    /// Fade between colors instead of jumping. When on, recent colors are batched into a
    /// single `start_cf` color flow (see [`AmbientSink::send_flow`]) so the bulb animates the
    /// whole window continuously; when off, each color is sent instantly (`sudden`).
    pub(crate) smooth: bool,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self {
            region: color::Region::default(),
            mode: color::ExtractMode::default(),
            monitor_id: None,
            main: true,
            bg: false,
            smooth: true,
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
    /// Ambient's send rate for this sink (music-mode vs rate-limited fallback). The audio
    /// driver reuses [`send`](Self::send) but paces it with its own higher music rate.
    pub(crate) fn fps(&self) -> u64 {
        if self.music.is_some() { MUSIC_FPS } else { FALLBACK_FPS }
    }

    /// Whether this sink streams over the (quota-free) music channel rather than the
    /// rate-limited direct client — the reused signal both drivers pace against.
    pub(crate) fn is_music(&self) -> bool {
        self.music.is_some()
    }

    /// Push `rgb` **instantly** (`sudden`) to the enabled targets — the non-smooth path. Each
    /// target uses its RGB method if supported, else color temperature (the color mapped to a
    /// warm/cool K). Over music: fire-and-forget; else the rate-limited direct client. The
    /// smooth path goes through [`send_flow`](Self::send_flow). Shared by both drivers.
    pub(crate) async fn send(&self, rgb: Rgb, main: bool, bg: bool) -> Result<(), String> {
        let v = rgb_value(rgb);
        let ct = rgb_to_ct(rgb);
        if let Some(music) = &self.music {
            let mut s = music.lock().await;
            let p = |value: u32| vec![json!(value), json!("sudden"), json!(0)];
            if main {
                if self.caps.main_rgb {
                    s.send("set_rgb", p(v)).await.map_err(|e| e.to_string())?;
                } else if self.caps.main_ct {
                    s.send("set_ct_abx", p(u32::from(ct))).await.map_err(|e| e.to_string())?;
                }
            }
            if bg {
                if self.caps.bg_rgb {
                    s.send("bg_set_rgb", p(v)).await.map_err(|e| e.to_string())?;
                } else if self.caps.bg_ct {
                    s.send("bg_set_ct_abx", p(u32::from(ct))).await.map_err(|e| e.to_string())?;
                }
            }
        } else {
            let eff = Effect::Sudden;
            if main {
                if self.caps.main_rgb {
                    self.client.set_rgb(v, eff).await.map_err(|e| e.to_string())?;
                } else if self.caps.main_ct {
                    self.client.set_ct_abx(ct, eff).await.map_err(|e| e.to_string())?;
                }
            }
            if bg {
                if self.caps.bg_rgb {
                    self.client.bg_set_rgb(v, eff).await.map_err(|e| e.to_string())?;
                } else if self.caps.bg_ct {
                    self.client.bg_set_ct_abx(ct, eff).await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    /// The **smooth** path: play `colors` as a single `start_cf` color flow, each held for
    /// `step_ms`, on the enabled targets. Batching a window of colors into one flow lets even
    /// a rate-limited bulb animate the whole span instead of jumping between throttled
    /// samples — the technique ported from the previous project's `RgbSender`. An RGB target
    /// uses a color flow; a white-only target uses a color-temperature flow. Over music:
    /// fire-and-forget `start_cf`; else the direct client. `FlowAction::Stay` holds the last
    /// color until the next flow arrives.
    pub(crate) async fn send_flow(
        &self,
        colors: &[Rgb],
        step_ms: u32,
        main: bool,
        bg: bool,
    ) -> Result<(), String> {
        if colors.is_empty() {
            return Ok(());
        }
        let count = colors.len() as u32;
        let rgb_expr = rgb_flow(colors, step_ms);
        let ct_expr = ct_flow(colors, step_ms);

        if let Some(music) = &self.music {
            // Over music, start_cf params are [count, action, flow_string]; action 1 = Stay.
            let mut s = music.lock().await;
            let main_expr = if self.caps.main_rgb {
                Some(&rgb_expr)
            } else if self.caps.main_ct {
                Some(&ct_expr)
            } else {
                None
            };
            if main && let Some(expr) = main_expr {
                let e = expr.render().map_err(|e| e.to_string())?;
                s.send("start_cf", vec![json!(count), json!(1), json!(e)])
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let bg_expr = if self.caps.bg_rgb {
                Some(&rgb_expr)
            } else if self.caps.bg_ct {
                Some(&ct_expr)
            } else {
                None
            };
            if bg && let Some(expr) = bg_expr {
                let e = expr.render().map_err(|e| e.to_string())?;
                s.send("bg_start_cf", vec![json!(count), json!(1), json!(e)])
                    .await
                    .map_err(|e| e.to_string())?;
            }
        } else {
            let cf = FlowAction::Stay;
            if main {
                if self.caps.main_rgb {
                    self.client.start_cf(count, cf, rgb_expr.clone()).await.map_err(|e| e.to_string())?;
                } else if self.caps.main_ct {
                    self.client.start_cf(count, cf, ct_expr.clone()).await.map_err(|e| e.to_string())?;
                }
            }
            if bg {
                if self.caps.bg_rgb {
                    self.client.bg_start_cf(count, cf, rgb_expr.clone()).await.map_err(|e| e.to_string())?;
                } else if self.caps.bg_ct {
                    self.client.bg_start_cf(count, cf, ct_expr.clone()).await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
}

/// Batches reactive colors into one `start_cf` color flow — the shared engine behind both
/// effect loops' smooth path. Rather than one throttled `set_rgb` per allowed command (which
/// jumps), it collects recent colors and, once a window is due, hands them back as a flow so
/// the whole span animates. Ported from the previous project's `RgbSender` batched fallback.
pub(crate) struct FlowBuf {
    buf: Vec<Rgb>,
    last: Instant,
}

impl FlowBuf {
    /// A fresh, empty buffer.
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new(), last: Instant::now() }
    }

    /// Buffer `color` (dropping the oldest past [`MAX_FLOW_STEPS`]). Returns `(colors,
    /// step_ms)` to flow when `gap` has elapsed since the last flush (or `force`), else `None`
    /// while the window is still filling. `step_ms` divides the real elapsed time across the
    /// buffered colors so the flow plays back at roughly real speed (floored at the 50ms flow
    /// minimum, capped at `gap`).
    pub(crate) fn push(&mut self, color: Rgb, gap: Duration, force: bool) -> Option<(Vec<Rgb>, u32)> {
        self.buf.push(color);
        if self.buf.len() > MAX_FLOW_STEPS {
            let cut = self.buf.len() - MAX_FLOW_STEPS;
            self.buf.drain(0..cut);
        }
        if !force && self.last.elapsed() < gap {
            return None; // still filling this window
        }
        if self.buf.is_empty() {
            return None;
        }
        let ms = self.last.elapsed().as_millis() as u32;
        // Floor at the 50ms flow minimum, cap at the window so one flow doesn't overrun it
        // (cap kept ≥ floor so a degenerate gap never inverts the clamp).
        let cap = (gap.as_millis() as u32).max(50);
        let step = (ms / self.buf.len() as u32).clamp(50, cap);
        let colors = std::mem::take(&mut self.buf);
        self.last = Instant::now();
        Some((colors, step))
    }
}

/// The `rgb` value to send, floored to 1. Yeelight's `set_rgb`/`bg_set_rgb` reject 0
/// (pure black) with `-5001 invalid params`, and both drivers reach 0 whenever the source
/// is silent/black (music-capture during quiet, ambient on a black screen). `0x000001` is
/// effectively off — the intended "nothing playing / dark screen" look — and is accepted.
fn rgb_value(rgb: Rgb) -> u32 {
    rgb.to_u32().max(1)
}

/// Build a color-flow expression driving an RGB target: one mode-1 (color) tuple per color,
/// each held `step_ms`. `brightness: -1` keeps the bulb's current level (the reactive
/// brightness is already baked into the RGB), matching the sudden path.
fn rgb_flow(colors: &[Rgb], step_ms: u32) -> FlowExpr {
    FlowExpr(
        colors
            .iter()
            .map(|&c| FlowTuple { duration: step_ms, mode: 1, value: rgb_value(c), brightness: -1 })
            .collect(),
    )
}

/// Build a color-flow expression for a white-only (CT) target: each color maps to a
/// mode-2 (color-temperature) tuple via [`rgb_to_ct`].
fn ct_flow(colors: &[Rgb], step_ms: u32) -> FlowExpr {
    FlowExpr(
        colors
            .iter()
            .map(|&c| FlowTuple {
                duration: step_ms,
                mode: 2,
                value: u32::from(rgb_to_ct(c)),
                brightness: -1,
            })
            .collect(),
    )
}

/// Map an RGB color to an approximate correlated color temperature (`1700..=6500` K) for
/// white-only (CT) bulbs: warm/red colors → low K, cool/blue → high K.
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
    // Capture no faster than we send — capturing ahead of the consume rate just burns CPU
    // on frames the driver dedups away (fallback is 2 fps, so 20 fps capture wasted 90%).
    let fps = sink.fps();
    let (rgb_tx, rgb_rx) = watch::channel(Rgb::BLACK);

    let guard = match capture::spawn(monitor_id, fps, cfg_rx.clone(), rgb_tx) {
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
        flow: FlowBuf, // batches colors into a start_cf on the smooth path
        _guard: capture::CaptureGuard, // stops capture on drop
    }

    let state = State {
        id,
        sink,
        cfg_rx,
        rgb_rx,
        // None: push the first captured frame unconditionally (so a genuinely dark screen
        // still drives the bulb). Both backends publish a real frame before `spawn` returns
        // Ok (readiness probe), so there's no phantom black.
        last_sent: None,
        powered_main: false,
        powered_bg: false,
        errored: false,
        flow: FlowBuf::new(),
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

            // Smooth = batch into a color flow (start_cf); instant = one sudden set_rgb.
            // `None` means the flow window is still filling — nothing sent this tick.
            let outcome: Option<Result<(), String>> = if cfg.smooth {
                let gap = if st.sink.music.is_some() { FLOW_GAP_MUSIC } else { FLOW_GAP_DIRECT };
                match st.flow.push(rgb, gap, force) {
                    Some((colors, step)) => Some(st.sink.send_flow(&colors, step, cfg.main, cfg.bg).await),
                    None => None,
                }
            } else {
                Some(st.sink.send(rgb, cfg.main, cfg.bg).await)
            };

            match outcome {
                None => st.last_sent = Some(rgb), // buffered — dedup future frames against it
                Some(Ok(())) => {
                    tracing::debug!("ambient: sent rgb={rgb:?} (main={} bg={})", cfg.main, cfg.bg);
                    st.last_sent = Some(rgb);
                    st.errored = false;
                }
                Some(Err(e)) => {
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
    fn flow_buf_batches_a_window_then_flushes() {
        let mut fb = FlowBuf::new();
        let c = |n: u8| Rgb { r: n, g: n, b: n };
        // Not yet due (gap not elapsed, not forced): keeps buffering.
        assert!(fb.push(c(1), Duration::from_secs(60), false).is_none());
        // Forced flush returns the buffered colors and a per-step duration ≥ 50ms.
        let (colors, step) = fb.push(c(2), Duration::from_secs(60), true).expect("forced flush");
        assert_eq!(colors, vec![c(1), c(2)]);
        assert!(step >= 50, "step floored at the 50ms flow minimum: {step}");
    }

    #[test]
    fn rgb_flow_renders_expected_wire_string() {
        // The exact `start_cf` payload a smooth transition sends: `dur,mode,value,bright`
        // per step. mode 1 = color, value = packed RGB, brightness -1 = keep current.
        let colors = [
            Rgb { r: 255, g: 0, b: 0 },
            Rgb { r: 0, g: 255, b: 0 },
            Rgb { r: 0, g: 0, b: 255 },
        ];
        let s = rgb_flow(&colors, 100).render().expect("valid flow");
        assert_eq!(s, "100,1,16711680,-1,100,1,65280,-1,100,1,255,-1");
    }

    #[test]
    fn ct_flow_maps_each_color_to_temperature() {
        // A white-only target flows color temperatures (mode 2), warm red → low CT.
        let s = ct_flow(&[Rgb { r: 255, g: 0, b: 0 }], 200).render().expect("valid flow");
        assert!(s.starts_with("200,2,"), "ct tuple: {s}");
        assert!(s.ends_with(",-1"), "brightness kept: {s}");
    }

    #[test]
    fn flow_buf_caps_at_max_steps() {
        let mut fb = FlowBuf::new();
        for n in 0..(MAX_FLOW_STEPS as u8 + 5) {
            let _ = fb.push(Rgb { r: n, g: 0, b: 0 }, Duration::from_secs(60), false);
        }
        let (colors, _) = fb.push(Rgb::BLACK, Duration::ZERO, true).expect("flush");
        assert_eq!(colors.len(), MAX_FLOW_STEPS, "window keeps only the most recent colors");
    }

    #[test]
    fn rgb_value_floors_black_to_one() {
        // Pure black (silence / dark screen) must never send 0 — the device rejects it
        // as -5001 invalid params. Any real color passes through unchanged.
        assert_eq!(rgb_value(Rgb::BLACK), 1);
        assert_eq!(rgb_value(Rgb { r: 0x12, g: 0x34, b: 0x56 }), 0x123456);
        assert_eq!(rgb_value(Rgb { r: 0, g: 0, b: 1 }), 1);
    }

    #[test]
    fn rgb_to_ct_maps_warm_low_cool_high() {
        assert!(rgb_to_ct(Rgb { r: 255, g: 0, b: 0 }) < 2000, "red is warm");
        assert!(rgb_to_ct(Rgb { r: 0, g: 0, b: 255 }) > 6000, "blue is cool");
        let grey = rgb_to_ct(Rgb { r: 128, g: 128, b: 128 });
        assert!((3500..=4700).contains(&grey), "grey is mid: {grey}");
    }
}
