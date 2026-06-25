//! `scap` wrapper: enumerate monitors and run a capture thread that publishes the
//! latest region color into a `watch` channel. Not unit-tested — needs a real display
//! and (on Wayland) the desktop screen-share portal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use scap::capturer::{Capturer, Options, Resolution};
use scap::frame::{Frame, FrameType};
use scap::Target;
use tokio::sync::watch;

use super::AmbientConfig;
use super::color::{self, Rgb};

/// A selectable display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Monitor {
    /// scap display id.
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
            Target::Window(_) => None,
        })
        .collect()
}

/// Signals the capture thread to stop when dropped.
// ponytail: detaches (no join) so dropping never blocks the UI thread. The thread checks
// the flag after each frame and exits within ~one frame (scap pushes at `fps`), then drops
// the Capturer (ending the OS session). Join only if a lingering session ever matters.
pub(crate) struct CaptureGuard {
    stop: Arc<AtomicBool>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start capturing `monitor_id` (None = primary). Each frame is cropped+reduced per the
/// latest `cfg` and the resulting color is published to `rgb_tx`. On Wayland the first
/// capture triggers the OS screen-share dialog (via the desktop portal).
///
/// Returns an error string if capture is unsupported or permission is denied — surface it.
/// (Note: on Linux `scap::has_permission()` is a placeholder returning `true`; the real
/// portal permission is requested by the capture engine when `start_capture` runs.)
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
        fps: 30, // capture rate; the driver downsamples to the send rate
        target,
        show_cursor: false,
        output_type: FrameType::BGRAFrame,
        output_resolution: Resolution::_480p, // average color needs no detail; less CPU
        ..Default::default()
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);

    // scap-rs's `Capturer` is `!Send` on Linux (its `LinuxCapturer` holds a
    // `Box<dyn LinuxCapturerImpl>` with no `Send` bound), so it cannot be built on the
    // UI thread and moved into the worker. Build it *inside* the worker (only the `Send`
    // `Options` crosses the boundary) and report the build result back over a oneshot
    // `mpsc` so `spawn` keeps its synchronous `Result<CaptureGuard, String>` contract.
    let (build_tx, build_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    // Detached: the guard signals stop; the thread exits on its own and drops the Capturer.
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
                        let stride = f.data.len() / h as usize; // tolerate row padding
                        let c = cfg.borrow().clone();
                        let bounds = color::crop_bounds(c.region, w, h);
                        let rgb = color::extract(&f.data, stride, bounds, c.mode);
                        // Ignore send error: receiver gone means we're shutting down.
                        let _ = rgb_tx.send(rgb);
                    }
                    Ok(_) => {} // non-BGRA frame (shouldn't happen with our FrameType) — skip
                    Err(_) => break, // channel closed: capturer stopped
                }
            }
            capturer.stop_capture();
        })
        .map_err(|e| e.to_string())?;

    // Surface a build failure (unsupported / permission denied) synchronously. A dropped
    // sender (thread panicked before sending) also reports as an error rather than hanging.
    match build_rx.recv() {
        Ok(Ok(())) => Ok(CaptureGuard { stop }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("capture thread exited before initializing".into()),
    }
}
