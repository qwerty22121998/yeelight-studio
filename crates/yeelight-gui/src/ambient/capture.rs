//! Screen capture: enumerate monitors and run a thread publishing the latest region
//! color into a `watch` channel. Not unit-tested — needs a real display.
//!
//! Linux/Wayland captures via the `grim` CLI (wlr-screencopy): scap's pipewire-portal
//! engine dies instantly on wlroots compositors (proven — 0 frames, closed channel).
//! Other platforms use `scap` (ScreenCaptureKit / DXGI), which works there.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::watch;

use super::AmbientConfig;
use super::color::{self, Rgb};

/// A selectable display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Monitor {
    /// Backend display id.
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
// ponytail: detaches (no join) so dropping never blocks the UI thread. The thread checks
// the flag between grabs and exits within one capture interval, then ends the session.
pub(crate) struct CaptureGuard {
    stop: Arc<AtomicBool>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ===== Linux / Wayland: grim (wlr-screencopy) =======================================

#[cfg(target_os = "linux")]
mod linux {
    use std::process::Command;
    use std::time::Duration;

    use super::*;

    /// How often the worker grabs a fresh frame. Kept ahead of the driver's send rate
    /// (≤15 fps) so a fresh color is always ready; the driver downsamples + dedups.
    // ponytail: one `grim` subprocess per grab — fine for a region at 20 fps. If a future
    // mode needs higher rates, switch to a persistent wlr-screencopy session.
    const CAPTURE_INTERVAL: Duration = Duration::from_millis(50);

    /// Logical geometry of a monitor (Wayland logical coords, as `grim -g` expects).
    #[derive(Clone, Copy)]
    struct Geom {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    }

    /// One entry of `hyprctl monitors -j`.
    #[derive(serde::Deserialize)]
    struct HyprMon {
        id: u32,
        name: String,
        x: i32,
        y: i32,
        /// Physical mode width/height; logical size is this divided by `scale`.
        width: u32,
        height: u32,
        #[serde(default = "one")]
        scale: f64,
        #[serde(default)]
        focused: bool,
    }

    fn one() -> f64 {
        1.0
    }

    fn query_monitors() -> Vec<HyprMon> {
        match Command::new("hyprctl").args(["monitors", "-j"]).output() {
            Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// All displays (windows excluded). Empty if `hyprctl` is unavailable.
    // ponytail: Hyprland-only enumeration. grim works on any wlroots compositor, but
    // monitor listing is compositor-specific — add `wlr-randr`/`swaymsg` if needed.
    pub(crate) fn monitors() -> Vec<Monitor> {
        query_monitors()
            .into_iter()
            .map(|m| Monitor { id: m.id, label: m.name })
            .collect()
    }

    pub(crate) fn spawn(
        monitor_id: Option<u32>,
        cfg: watch::Receiver<AmbientConfig>,
        rgb_tx: watch::Sender<Rgb>,
    ) -> Result<CaptureGuard, String> {
        let mons = query_monitors();
        if mons.is_empty() {
            return Err("could not enumerate monitors (need `hyprctl`; Hyprland only)".into());
        }
        let m = monitor_id
            .and_then(|id| mons.iter().find(|m| m.id == id))
            .or_else(|| mons.iter().find(|m| m.focused))
            .or_else(|| mons.first())
            .expect("mons is non-empty");
        let logical = |px: u32| ((px as f64 / m.scale).round() as u32).max(1);
        let geom = Geom { x: m.x, y: m.y, w: logical(m.width), h: logical(m.height) };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        // The first grab doubles as a readiness probe: report success/failure over this
        // channel so `spawn` keeps its synchronous `Result` (and surfaces an install hint).
        let (build_tx, build_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new()
            .name("ambient-capture".into())
            .spawn(move || {
                match grab(geom, &cfg) {
                    Ok(rgb) => {
                        let _ = rgb_tx.send(rgb);
                        let _ = build_tx.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = build_tx.send(Err(e));
                        return;
                    }
                }
                tracing::info!("ambient capture: grim started (monitor={monitor_id:?})");
                let mut frames: u64 = 1;
                while !stop_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(CAPTURE_INTERVAL);
                    match grab(geom, &cfg) {
                        Ok(rgb) => {
                            frames += 1;
                            if frames % 60 == 1 {
                                tracing::debug!("ambient capture: frame {frames} rgb={rgb:?}");
                            }
                            let _ = rgb_tx.send(rgb);
                        }
                        // Transient failure (compositor busy): keep trying — don't kill the
                        // stream the way the old pipewire path did on its first hiccup.
                        Err(e) => tracing::warn!("ambient capture: grim grab failed: {e}"),
                    }
                }
                tracing::info!("ambient capture: stopped after {frames} frames");
            })
            .map_err(|e| e.to_string())?;

        match build_rx.recv() {
            Ok(Ok(())) => Ok(CaptureGuard { stop }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("capture thread exited before initializing".into()),
        }
    }

    /// Capture the configured region once via `grim` and reduce it to a color.
    fn grab(mon: Geom, cfg: &watch::Receiver<AmbientConfig>) -> Result<Rgb, String> {
        let c = cfg.borrow().clone();
        let (cx, cy, cw, ch) = color::crop_bounds(c.region, mon.w, mon.h);
        let geo = format!("{},{} {}x{}", mon.x + cx as i32, mon.y + cy as i32, cw, ch);
        let out = Command::new("grim")
            .args(["-g", &geo, "-t", "ppm", "-"])
            .output()
            .map_err(|e| format!("`grim` not runnable ({e}); install grim"))?;
        if !out.status.success() {
            return Err(format!(
                "grim exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let (w, h, pixels) = parse_ppm(&out.stdout)?;
        // grim already cropped to the region; reduce the whole returned image.
        Ok(color::extract_rgb(pixels, w * 3, (0, 0, w as u32, h as u32), c.mode))
    }

    /// Parse a binary `P6` PPM header and return `(width, height, pixel_bytes)`.
    fn parse_ppm(data: &[u8]) -> Result<(usize, usize, &[u8]), String> {
        if data.get(0..2) != Some(b"P6") {
            return Err("ppm: not P6".into());
        }
        let mut p = 2usize;
        let mut nums = [0usize; 3]; // width, height, maxval
        for slot in &mut nums {
            // Skip whitespace and `#` comments before each number.
            loop {
                match data.get(p) {
                    Some(b'#') => {
                        while data.get(p).is_some_and(|&b| b != b'\n') {
                            p += 1;
                        }
                    }
                    Some(b) if b.is_ascii_whitespace() => p += 1,
                    _ => break,
                }
            }
            let start = p;
            while data.get(p).is_some_and(u8::is_ascii_digit) {
                p += 1;
            }
            *slot = std::str::from_utf8(&data[start..p])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or("ppm: malformed header")?;
        }
        p += 1; // single whitespace byte after maxval, then pixel data
        let (w, h) = (nums[0], nums[1]);
        let pixels = data.get(p..p + w * h * 3).ok_or("ppm: truncated pixel data")?;
        Ok((w, h, pixels))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_ppm_reads_header_and_pixels() {
            // 2x1 image: red, green.
            let mut data = b"P6\n2 1\n255\n".to_vec();
            data.extend_from_slice(&[255, 0, 0, 0, 255, 0]);
            let (w, h, px) = parse_ppm(&data).unwrap();
            assert_eq!((w, h), (2, 1));
            assert_eq!(px, &[255, 0, 0, 0, 255, 0]);
        }

        #[test]
        fn parse_ppm_rejects_truncated() {
            let data = b"P6\n2 1\n255\n\xff\x00".to_vec(); // needs 6 bytes, has 2
            assert!(parse_ppm(&data).is_err());
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{monitors, spawn};

// ===== Other platforms: scap (ScreenCaptureKit / DXGI) ==============================

#[cfg(not(target_os = "linux"))]
mod other {
    use scap::Target;
    use scap::capturer::{Capturer, Options, Resolution};
    use scap::frame::{Frame, FrameType};

    use super::*;

    /// All displays scap can see (windows excluded). Empty if capture is unsupported.
    pub(crate) fn monitors() -> Vec<Monitor> {
        if !scap::is_supported() {
            return Vec::new();
        }
        scap::get_all_targets()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| match t {
                Target::Display(d) => Some(Monitor { id: d.id, label: d.title }),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn spawn(
        monitor_id: Option<u32>,
        cfg: watch::Receiver<AmbientConfig>,
        rgb_tx: watch::Sender<Rgb>,
    ) -> Result<CaptureGuard, String> {
        if !scap::is_supported() {
            return Err("screen capture not supported on this platform".into());
        }
        if !scap::has_permission() && !scap::request_permission() {
            return Err("screen-capture permission denied (approve the screen-share dialog)".into());
        }

        let target = monitor_id.and_then(|id| {
            scap::get_all_targets()
                .ok()?
                .into_iter()
                .find(|t| matches!(t, Target::Display(d) if d.id == id))
        });

        let options = Options {
            fps: 30,
            target,
            show_cursor: false,
            output_type: FrameType::BGRAFrame,
            output_resolution: Resolution::_480p,
            ..Default::default()
        };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        // scap's `Capturer` is `!Send` on some platforms, so build it inside the worker.
        let (build_tx, build_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new()
            .name("ambient-capture".into())
            .spawn(move || {
                let mut capturer = match Capturer::build(options) {
                    Ok(c) => {
                        let _ = build_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = build_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                capturer.start_capture();
                while !stop_thread.load(Ordering::Relaxed) {
                    match capturer.get_next_frame() {
                        Ok(Frame::BGRA(f)) => {
                            let (w, h) = (f.width.max(0) as u32, f.height.max(0) as u32);
                            if w == 0 || h == 0 || f.data.is_empty() {
                                continue;
                            }
                            let stride = f.data.len() / h as usize;
                            let c = cfg.borrow().clone();
                            let bounds = color::crop_bounds(c.region, w, h);
                            let rgb = color::extract(&f.data, stride, bounds, c.mode);
                            let _ = rgb_tx.send(rgb);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                capturer.stop_capture();
            })
            .map_err(|e| e.to_string())?;

        match build_rx.recv() {
            Ok(Ok(())) => Ok(CaptureGuard { stop }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("capture thread exited before initializing".into()),
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) use other::{monitors, spawn};
