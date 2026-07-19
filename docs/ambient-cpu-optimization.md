# Ambient mode CPU optimization

Report on reducing the CPU cost of the GUI's ambient screen-capture light.

## Problem

Ambient mode (`yeelight-gui/src/ambient/`) mirrors a screen region's color onto
the bulb. On the Linux/Wayland path it captures via the `grim` CLI — one
subprocess per grab, each doing a full compositor framebuffer copy + PPM encode.
The capture thread ran at a **fixed rate independent of how fast colors were
actually sent** to the bulb, so most captured frames were thrown away.

## Root cause (measured, not assumed)

| What | Measurement |
|---|---|
| GUI idle, ambient **off** | **0.0%** of one core — iced is event-driven, not a CPU hog |
| One full 2560×1440 `grim` grab | ~0.02s CPU, 11 MB PPM output |
| Capture rate (old) | 20 fps (Linux) / 30 fps (scap) — **fixed** |
| Send/consume rate | 15 fps (music mode) or **2 fps (fallback, common case)** |

The capture loop grabbed at 20 fps while the driver consumed at 2 fps in the
common (no-music) case: **~90% of every grab was captured and immediately
deduped away.** grim at 20 fps ≈ ~40% of one core; iced and color extraction
were negligible by comparison.

## Change

Capture no faster than the sink actually sends. The send rate
(`AmbientSink::fps()`) is now threaded into `capture::spawn()`, and both capture
backends derive their rate from it instead of a hardcoded constant.

### Old

```rust
// capture.rs (linux)
const CAPTURE_INTERVAL: Duration = Duration::from_millis(50); // 20 fps, fixed
...
pub(crate) fn spawn(monitor_id, cfg, rgb_tx) -> Result<CaptureGuard, String> {
    ...
    std::thread::sleep(CAPTURE_INTERVAL);
}

// capture.rs (scap / non-linux)
let options = Options { fps: 30, ... }; // fixed

// mod.rs
let guard = capture::spawn(monitor_id, cfg_rx.clone(), rgb_tx)?;
```

### New

```rust
// mod.rs — pass the sink's send rate into capture
let fps = sink.fps();                       // 15 (music) or 2 (fallback)
let guard = capture::spawn(monitor_id, fps, cfg_rx.clone(), rgb_tx)?;

// capture.rs (linux) — interval derived from fps
let interval = Duration::from_millis(1000 / fps.max(1));
...
std::thread::sleep(interval);

// capture.rs (scap / non-linux)
let options = Options { fps: fps.max(1) as u32, ... };
```

Files touched: `crates/yeelight-gui/src/ambient/mod.rs`,
`crates/yeelight-gui/src/ambient/capture.rs` (+14 / −9).

## Result

| Scenario | Capture rate | grim CPU (approx, one core) |
|---|---|---|
| **Old** — fallback (no music) | 20 fps | ~40% |
| **New** — fallback (no music) | 2 fps | **~4%** |
| **New** — music mode | 15 fps | ~30% (unchanged; frames now all used) |

For the common no-music case, ambient's dominant cost dropped **~40% → ~5% of
one core** (grim ~4% + ~1% in-process parse/average). Idle stays 0.0%.

Responsiveness is unchanged: the driver already only sends at 2 fps in fallback,
so capturing faster never improved anything visible. A captured color is now at
most one send-period stale — imperceptible for mood lighting.

## Alternatives rejected (proven, not assumed)

- **grim `-s` downscale** (shrink the frame before it hits our pipe): *worse.*
  grim scales in software, so CPU per grab jumped **0.01s → 0.12s (~12×)**. The
  full raw grab is the cheapest option; the transfer/parse savings don't come
  close to covering the scaling cost.
- **iced repaint tuning**: nothing to fix — idle measured at 0.0%.
- **Persistent `wlr-screencopy` session** (avoid the per-grab subprocess
  entirely): a real win at high frame rates, but a real rewrite (wayland-client
  + protocol code). Not worth it at 2 fps; flagged in a `ponytail:` comment as
  the upgrade path if a future high-fps mode ever needs it.

## Remaining dial

The only lever left without the rewrite is frame rate. Dropping the fallback
constant `FALLBACK_FPS` from 2 → 1 halves grim's cost again (~4% → ~2%) at the
price of slightly laggier reaction. Left at 2 as the current default.

## Verification

- `cargo build -p yeelight-gui` — clean.
- `cargo test -p yeelight-gui` — passed.
- Ran the GUI against a real bulb: ambient streamed correctly at the new rate
  (`grim started`, `bg_set_rgb` streaming in the logs).
- CPU numbers above measured on the running process via `/proc/<pid>/stat`
  (per-thread) and `grim` timed standalone with the shell `time` builtin.

---

# Part 2 — Rust-native capture: grim/scap → xcap + libwayshot

Follow-up to Part 1. Goal: drop the external `grim` CLI (+`hyprctl`) and the
**unmaintained** `scap` crate for a fully Rust-native, cross-platform capture
path — without regressing the CPU win from Part 1.

## What shipped

Two Rust-native backends, selected at runtime (`capture.rs`,
`prefer_libwayshot()` cached in a `OnceLock`):

- **libwayshot 0.8** (`zwlr_screencopy_v1`) — chosen on wlroots compositors
  (Hyprland/sway/river): whenever `WAYLAND_DISPLAY` is set and a
  `WayshotConnection` opens.
- **xcap 0.9** — everywhere else: Linux X11, macOS (ScreenCaptureKit),
  Windows (DXGI/WGC), and portal-based Wayland (GNOME/KDE).

Both yield packed **RGBA**, so the two old extractors (`extract` BGRA + `extract_rgb`
RGB) collapsed into one `extract_rgba`. No subprocess, no CLI dependency, no
`hyprctl`. The public `spawn`/`monitors`/`CaptureGuard` contract is unchanged, so
`mod.rs`/`app.rs`/the view were untouched.

## Why not "xcap everywhere" (the original idea)

The plan started as *xcap default, libwayshot fallback*. A measurement spike on
this Hyprland box (a throwaway example, since none of this is reliably documented)
**inverted that decision** — xcap goes through pipewire/portal here, which is far
heavier than xcap's own bundled wlr-screencopy path:

| Backend (release, 2 fps, 2560×1440) | First frame | CPU (one core) | Path |
|---|---|---|---|
| **xcap** | 611 ms | 4.4% | pipewire/portal |
| **libwayshot** | **21 ms** | **1.1%** | wlr-screencopy |
| grim (Part 1 baseline) | ~28 ms | ~4% | wlr-screencopy (subprocess) |

So on wlroots, **libwayshot wins decisively** (4× less CPU, 30× faster startup) —
and even beats the grim baseline. Hence: libwayshot on wlroots, xcap as the
cross-platform default elsewhere.

## Old vs new

| | Old | New |
|---|---|---|
| Linux/wlroots capture | `grim` CLI subprocess per grab | `libwayshot` in-process |
| Linux monitor enumeration | `hyprctl monitors -j` (Hyprland only) | `libwayshot` outputs (any wlroots) |
| macOS/Windows capture | `scap-rs 0.1` (**unmaintained**) | `xcap 0.9` (maintained) |
| Linux X11 / GNOME-KDE Wayland | *unsupported* | `xcap 0.9` |
| Pixel path | `extract` (BGRA) + `extract_rgb` (RGB) | one `extract_rgba` |
| External runtime deps | `grim`, `hyprctl` binaries | none (pure Rust) |
| wlroots CPU @ 2 fps | ~4% of a core | **~1% of a core** |

## Gotcha: debug builds

libwayshot/xcap/image do per-pixel work that is ~10× slower unoptimized. A plain
debug `cargo run` measured **~25% of a core**; release **~1%**. Fix: the workspace
root pins `opt-level = 3` for `libwayshot`, `image`, and `xcap` in the dev profile
only, so `cargo run` feels like release while the rest of the app stays debuggable.

## Cost paid

- **Heavier build**: xcap pulls `pipewire`, `zbus`, `xcb`, `libwayshot-xcap`,
  `image`; needs system `libpipewire-0.3` + `libxcb` dev headers (grim needed
  none). Bigger binary, longer cold build. Accepted for the broadened OS coverage.
- **Monitor ids** are backend-native (xcap ids vs libwayshot output indices);
  consistent because one cached backend does both enumeration and capture.

## Verification

- `cargo build -p yeelight-gui`, `cargo test -p yeelight-gui`, `cargo clippy
  --all-targets` — all clean.
- Display-dependent smoke test (`capture::livetest`, `#[ignore]`d) drives the real
  wired path: `monitors()` → `[{id:0,"DP-1"},{id:1,"HDMI-A-1"}]` (libwayshot
  selected), `spawn` publishes a real non-black frame. Run with
  `cargo test -p yeelight-gui capture::livetest -- --ignored --nocapture`.
- Backend CPU/latency measured via the spike example (`/proc/self/stat` deltas),
  release build, 2 fps.
