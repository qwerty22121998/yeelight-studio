//! Screen capture: enumerate monitors and run a thread publishing the latest region
//! color into a `watch` channel. Not unit-tested — needs a real display.
//!
//! Two Rust-native backends, picked at runtime:
//! - **libwayshot** (`zwlr_screencopy_v1`) on wlroots compositors (Hyprland/sway/river).
//!   Measured ~1% of a core at 2 fps here — ~4× cheaper and ~30× faster to first frame
//!   than xcap's pipewire/portal path, so it is preferred whenever it connects.
//! - **xcap** everywhere else (Linux X11, macOS ScreenCaptureKit, Windows DXGI/WGC, and
//!   portal-based Wayland). The maintained cross-platform default.
//!
//! Both deliver a packed **RGBA** frame reduced by [`color::extract_rgba`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use tokio::sync::watch;

use super::AmbientConfig;
use super::color::{self, Rgb};

/// A selectable display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Monitor {
    /// Backend display id (xcap monitor id, or libwayshot output index).
    pub(crate) id: u32,
    /// Human label for the picker.
    pub(crate) label: String,
}

impl std::fmt::Display for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.label.is_empty() {
            write!(f, "Display {}", self.id)
        } else {
            f.write_str(&self.label)
        }
    }
}

/// Signals the capture thread to stop when dropped.
pub(crate) struct CaptureGuard {
    stop: Arc<AtomicBool>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// On Linux, prefer libwayshot when the compositor speaks `zwlr_screencopy_v1` (wlroots).
/// The probe opens and drops one Wayland connection; the result is cached so `monitors()`
/// and `spawn()` always agree on the backend (keeping monitor ids consistent).
#[cfg(target_os = "linux")]
fn prefer_libwayshot() -> bool {
    use std::sync::OnceLock;
    static PREFER: OnceLock<bool> = OnceLock::new();
    *PREFER.get_or_init(|| {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            && libwayshot::WayshotConnection::new().is_ok()
    })
}

/// All displays (windows excluded). Empty if the backend can't enumerate.
pub(crate) fn monitors() -> Vec<Monitor> {
    #[cfg(target_os = "linux")]
    if prefer_libwayshot() {
        return libwayshot_monitors();
    }
    xcap_monitors()
}

/// Spawn the capture worker for `monitor_id`, publishing the latest region color into
/// `rgb_tx` at `fps`. Returns once the first frame is captured (readiness probe), or an
/// error if the backend can't start.
pub(crate) fn spawn(
    monitor_id: Option<u32>,
    fps: u64,
    cfg: watch::Receiver<AmbientConfig>,
    rgb_tx: watch::Sender<Rgb>,
) -> Result<CaptureGuard, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    // The first grab doubles as a readiness probe: the worker reports success/failure over
    // this channel so `spawn` stays synchronous and can surface an error to the UI.
    let (build_tx, build_rx) = mpsc::channel::<Result<(), String>>();
    let interval = Duration::from_millis(1000 / fps.max(1));

    std::thread::Builder::new()
        .name("ambient-capture".into())
        .spawn(move || {
            #[cfg(target_os = "linux")]
            if prefer_libwayshot() {
                run_libwayshot(monitor_id, interval, cfg, rgb_tx, stop_thread, build_tx);
                return;
            }
            run_xcap(monitor_id, interval, cfg, rgb_tx, stop_thread, build_tx);
        })
        .map_err(|e| e.to_string())?;

    match build_rx.recv() {
        Ok(Ok(())) => Ok(CaptureGuard { stop }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("capture thread exited before initializing".into()),
    }
}

// ===== xcap backend (all platforms) =================================================

fn xcap_monitors() -> Vec<Monitor> {
    xcap::Monitor::all()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| Some(Monitor { id: m.id().ok()?, label: m.name().unwrap_or_default() }))
        .collect()
}

fn run_xcap(
    monitor_id: Option<u32>,
    interval: Duration,
    cfg: watch::Receiver<AmbientConfig>,
    rgb_tx: watch::Sender<Rgb>,
    stop: Arc<AtomicBool>,
    build_tx: mpsc::Sender<Result<(), String>>,
) {
    let monitors = match xcap::Monitor::all() {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => {
            let _ = build_tx.send(Err("no monitors found".into()));
            return;
        }
        Err(e) => {
            let _ = build_tx.send(Err(e.to_string()));
            return;
        }
    };
    // Pick by id, else the primary, else the first.
    let mon = monitor_id
        .and_then(|id| monitors.iter().find(|m| m.id().ok() == Some(id)))
        .or_else(|| monitors.iter().find(|m| m.is_primary().unwrap_or(false)))
        .or_else(|| monitors.first())
        .expect("monitors non-empty");

    let grab = |m: &xcap::Monitor, cfg: &watch::Receiver<AmbientConfig>| -> Result<Rgb, String> {
        let c = cfg.borrow().clone();
        let img = m.capture_image().map_err(|e| e.to_string())?;
        let (w, h) = (img.width(), img.height());
        let bounds = color::crop_bounds(c.region, w, h);
        Ok(color::extract_rgba(img.as_raw(), w as usize * 4, bounds, c.mode))
    };

    capture_loop(interval, &rgb_tx, &stop, &build_tx, "xcap", || grab(mon, &cfg));
}

// ===== libwayshot backend (Linux / wlroots) =========================================

#[cfg(target_os = "linux")]
fn libwayshot_monitors() -> Vec<Monitor> {
    let Ok(conn) = libwayshot::WayshotConnection::new() else {
        return Vec::new();
    };
    conn.get_all_outputs()
        .iter()
        .enumerate()
        .map(|(i, o)| Monitor { id: i as u32, label: o.name.clone() })
        .collect()
}

#[cfg(target_os = "linux")]
fn run_libwayshot(
    monitor_id: Option<u32>,
    interval: Duration,
    cfg: watch::Receiver<AmbientConfig>,
    rgb_tx: watch::Sender<Rgb>,
    stop: Arc<AtomicBool>,
    build_tx: mpsc::Sender<Result<(), String>>,
) {
    let conn = match libwayshot::WayshotConnection::new() {
        Ok(c) => c,
        Err(e) => {
            let _ = build_tx.send(Err(e.to_string()));
            return;
        }
    };
    if conn.get_all_outputs().is_empty() {
        let _ = build_tx.send(Err("no wayland outputs found".into()));
        return;
    }
    let idx = monitor_id
        .map(|id| id as usize)
        .filter(|&i| i < conn.get_all_outputs().len())
        .unwrap_or(0);

    let grab = |cfg: &watch::Receiver<AmbientConfig>| -> Result<Rgb, String> {
        let c = cfg.borrow().clone();
        let out = &conn.get_all_outputs()[idx];
        let img = conn
            .screenshot_single_output(out, false)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (w, h) = (img.width(), img.height());
        let bounds = color::crop_bounds(c.region, w, h);
        Ok(color::extract_rgba(img.as_raw(), w as usize * 4, bounds, c.mode))
    };

    capture_loop(interval, &rgb_tx, &stop, &build_tx, "libwayshot", || grab(&cfg));
}

// ===== shared loop ==================================================================

/// First grab is the readiness probe (publish a real frame, then report `Ok`); after that,
/// grab every `interval` until stopped, publishing each color and logging every 60th frame.
/// A transient grab error is logged and retried (a busy compositor shouldn't kill the stream).
fn capture_loop(
    interval: Duration,
    rgb_tx: &watch::Sender<Rgb>,
    stop: &AtomicBool,
    build_tx: &mpsc::Sender<Result<(), String>>,
    backend: &str,
    mut grab: impl FnMut() -> Result<Rgb, String>,
) {
    match grab() {
        Ok(rgb) => {
            let _ = rgb_tx.send(rgb);
            let _ = build_tx.send(Ok(()));
        }
        Err(e) => {
            let _ = build_tx.send(Err(e));
            return;
        }
    }
    tracing::info!("ambient capture: {backend} started");
    let mut frames: u64 = 1;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(interval);
        match grab() {
            Ok(rgb) => {
                frames += 1;
                if frames % 60 == 1 {
                    tracing::debug!("ambient capture: frame {frames} rgb={rgb:?}");
                }
                let _ = rgb_tx.send(rgb);
            }
            Err(e) => tracing::warn!("ambient capture: {backend} grab failed: {e}"),
        }
    }
    tracing::info!("ambient capture: {backend} stopped after {frames} frames");
}

#[cfg(test)]
mod livetest {
    //! Display-dependent smoke test for the capture wiring — `#[ignore]`d so CI skips it
    //! (the rest of the suite is socket-free/display-free per repo convention). Run manually:
    //! `cargo test -p yeelight-gui capture::livetest -- --ignored --nocapture`
    use super::*;

    #[test]
    #[ignore = "needs a real display"]
    fn captures_a_frame() {
        let mons = monitors();
        println!("monitors(): {mons:?}");
        assert!(!mons.is_empty(), "no monitors enumerated");

        let (tx, rx) = watch::channel(Rgb::BLACK);
        let (_cfg_tx, cfg_rx) = watch::channel(AmbientConfig::default());
        let guard = spawn(None, 2, cfg_rx, tx).expect("spawn failed");
        let rgb = *rx.borrow(); // spawn returns only after the readiness probe grabbed a frame
        println!("first frame rgb = {rgb:?}");
        drop(guard);
        assert!(rgb != Rgb::BLACK, "captured frame was black");
    }
}
