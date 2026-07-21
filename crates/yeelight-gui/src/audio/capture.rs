//! Audio capture: open an input device with `cpal` and run a thread publishing the
//! latest spectrum bars into a `watch` channel. Not unit-tested — needs a real
//! audio device (see the `#[ignore]`d live smoke test); the DSP it feeds is tested
//! in [`super::dsp`].
//!
//! Rust-native and cross-platform, the audio analog of the ambient screen backend:
//! `cpal` binds ALSA (Linux), CoreAudio (macOS) and WASAPI (Windows) directly — no
//! subprocess, no external visualizer. The chosen input's samples are folded to mono
//! and fed to [`super::dsp::Analyzer`] every `FFT_SIZE`; the resulting bars drive both
//! the bulb color and the cava wave.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use tokio::sync::watch;

use super::AudioConfig;
use super::dsp::{Analyzer, FFT_SIZE};

/// Signals the capture thread to stop when dropped. Detaches (no join) so dropping
/// never blocks the UI thread; the thread notices the flag within one recv timeout.
pub(crate) struct CaptureGuard {
    stop: Arc<AtomicBool>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// All input devices the host reports, by name (includes microphones and, where the
/// audio server exposes them, `Monitor of …` loopback sources). Empty if enumeration fails.
pub(crate) fn inputs() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        // cpal 0.18: `Device: Display` — the name is its `to_string()`.
        Ok(devs) => devs.map(|d| d.to_string()).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "audio: input enumeration failed");
            Vec::new()
        }
    }
}

/// Spawn the capture worker for `input` (device name; `None` = host default input),
/// publishing the latest spectrum bars into `bars_tx`. Returns once the stream is
/// playing (readiness probe), or an error string if the device can't start.
pub(crate) fn spawn(
    input: Option<String>,
    bars_tx: watch::Sender<Vec<f32>>,
    cfg_rx: watch::Receiver<AudioConfig>,
) -> Result<CaptureGuard, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    // The stream-start doubles as a readiness probe: the worker reports success/failure
    // over this channel so `spawn` stays synchronous and can surface an error to the UI.
    let (build_tx, build_rx) = mpsc::channel::<Result<(), String>>();

    std::thread::Builder::new()
        .name("music-capture".into())
        .spawn(move || run(input, bars_tx, stop_thread, build_tx, cfg_rx))
        .map_err(|e| e.to_string())?;

    match build_rx.recv() {
        Ok(Ok(())) => Ok(CaptureGuard { stop }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("audio capture thread exited before initializing".into()),
    }
}

/// An opened, playing input stream plus the mono-sample receiver and its rate.
type OpenStream = (cpal::Stream, mpsc::Receiver<Vec<f32>>, f32);

/// A useless null capture PCM (opens fine but only yields zero samples).
fn is_null_device(name: &str) -> bool {
    name.contains("discard all samples") || name.contains("generate zero")
}

/// An ALSA plugin / pseudo-device (resampler, effects, mixdown, JACK/OSS bridge, the ALSA
/// "default" alias). These often open but deliver flaky, resampled, or silent audio, so
/// they're a last resort behind real hardware. `name` must be lowercased.
fn is_pseudo_device(name: &str) -> bool {
    is_null_device(name)
        || [
            "rate converter", "resampler", "plugin using", "plugin for", "upmix", "downmix",
            "speex", "jack audio", "open sound system", "default alsa", "surround",
        ]
        .iter()
        .any(|p| name.contains(p))
}

/// Candidate capture devices to try, in preference order. Many entries an audio host reports
/// can't actually be opened for capture (the ALSA sound-server PCMs frequently fail under
/// PipeWire) or open but are useless (null sinks, resampler/effect plugins), so the caller
/// tries these in turn until one truly opens. An explicit user pick is returned alone — if it
/// fails we surface that, rather than silently capturing a different device.
fn candidate_devices(host: &cpal::Host, input: Option<&str>) -> Vec<cpal::Device> {
    let all: Vec<cpal::Device> = host.input_devices().map(Iterator::collect).unwrap_or_default();
    if let Some(name) = input {
        return all.into_iter().filter(|d| d.to_string() == name).collect();
    }
    let name = |d: &cpal::Device| d.to_string().to_lowercase();
    let is_server = |n: &str| n.contains("pipewire") || n.contains("pulse");
    let mut out: Vec<cpal::Device> = Vec::new();
    // 1. Sound servers first — they capture the system default source / monitor when they work.
    out.extend(all.iter().filter(|d| is_server(&name(d))).cloned());
    // 2. The host's declared default input.
    out.extend(host.default_input_device());
    // 3. Real hardware (not a sound server, not a plugin/pseudo device) — the reliable source.
    out.extend(all.iter().filter(|d| { let n = name(d); !is_server(&n) && !is_pseudo_device(&n) }).cloned());
    // 4. Plugin/pseudo devices as a last resort (excluding the useless null sinks entirely).
    out.extend(all.into_iter().filter(|d| { let n = name(d); is_pseudo_device(&n) && !is_null_device(&n) }));
    out
}

/// Try to open + play `device` for capture, returning the stream, sample receiver and rate.
/// A device that can't produce its default config, uses an unhandled sample format, or fails
/// to build/play yields `Err` so the caller can move to the next candidate.
fn open_stream(device: &cpal::Device) -> Result<OpenStream, String> {
    let supported = device.default_input_config().map_err(|e| format!("input config: {e}"))?;
    let sample_format = supported.sample_format();
    let channels = supported.channels() as usize;
    let sample_rate = supported.sample_rate() as f32; // cpal 0.18: SampleRate = u32
    let config: cpal::StreamConfig = supported.into();

    // Audio callback → worker: mono sample batches. `mpsc` per-batch allocation is fine off
    // the realtime path (we only average+forward in the callback, no FFT there).
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let err_fn = |e| tracing::warn!(error = %e, "audio stream error");
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build::<f32>(device, &config, channels, tx, err_fn),
        cpal::SampleFormat::I16 => build::<i16>(device, &config, channels, tx, err_fn),
        cpal::SampleFormat::U16 => build::<u16>(device, &config, channels, tx, err_fn),
        other => Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    Ok((stream, rx, sample_rate))
}

/// Worker: open the first usable input device (whose callback forwards mono samples over an
/// mpsc), then loop analyzing `FFT_SIZE`-sample windows into bars until stopped. The
/// `cpal::Stream` lives on this thread for the loop's duration.
fn run(
    input: Option<String>,
    bars_tx: watch::Sender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    build_tx: mpsc::Sender<Result<(), String>>,
    mut cfg_rx: watch::Receiver<AudioConfig>,
) {
    // With no explicit device (auto), capture the *monitor of the default sink* — i.e.
    // whatever's playing (system audio) — instead of the sound server's default source,
    // which is the microphone. cpal-ALSA can't enumerate PipeWire/Pulse monitors as
    // devices, so we steer the sound-server capture via libpulse's PULSE_SOURCE. The
    // `@DEFAULT_MONITOR@` token auto-resolves to the current default sink's monitor.
    #[cfg(target_os = "linux")]
    if input.is_none() && std::env::var_os("PULSE_SOURCE").is_none() {
        // SAFETY: set once from this worker before any capture stream is opened; no other
        // thread reads PULSE_SOURCE concurrently in this app.
        unsafe { std::env::set_var("PULSE_SOURCE", "@DEFAULT_MONITOR@") };
    }

    let host = cpal::default_host();
    let candidates = candidate_devices(&host, input.as_deref());
    if candidates.is_empty() {
        let _ = build_tx.send(Err("no audio input device available".into()));
        return;
    }
    let mut last_err = "no audio input device could be opened".to_string();
    let mut opened: Option<OpenStream> = None;
    for device in candidates {
        let label = device.to_string();
        match open_stream(&device) {
            Ok(o) => {
                tracing::info!(device = %label, "music capture: opened input");
                opened = Some(o);
                break;
            }
            Err(e) => {
                tracing::debug!(device = %label, error = %e, "music capture: input unusable, trying next");
                last_err = format!("{label}: {e}");
            }
        }
    }
    let Some((_stream, samples_rx, sample_rate)) = opened else {
        let _ = build_tx.send(Err(last_err));
        return;
    };
    let _ = build_tx.send(Ok(())); // ready — the UI can show "running"
    tracing::info!(%sample_rate, "music capture started");

    let mut analyzer = {
        let c = cfg_rx.borrow_and_update();
        Analyzer::new(sample_rate, c.bars, c.gain, c.decay)
    };
    let mut ring: Vec<f32> = Vec::with_capacity(FFT_SIZE * 2);
    let mut frames: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        // Live "Tune" edits (Sensitivity / Smoothing / Bars) — apply without dropping the
        // stream, so dragging a slider reshapes the wave in real time.
        if cfg_rx.has_changed().unwrap_or(false) {
            let c = cfg_rx.borrow_and_update();
            analyzer.set_params(c.bars, c.gain, c.decay);
        }
        match samples_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(batch) => {
                ring.extend_from_slice(&batch);
                if ring.len() >= FFT_SIZE {
                    analyzer.analyze(&ring);
                    let _ = bars_tx.send(analyzer.bars().to_vec());
                    frames += 1;
                    if frames % 200 == 1 {
                        tracing::trace!("music capture: frame {frames}");
                    }
                    // Keep only the most recent window so memory stays bounded.
                    let keep = ring.len() - FFT_SIZE;
                    ring.drain(0..keep);
                }
            }
            // Silence/underrun: publish a zeroed frame (at the current bar count) so the
            // wave settles.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = bars_tx.send(vec![0.0; analyzer.bars().len()]);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::info!("music capture stopped after {frames} frames");
}

/// Build an input stream for sample type `T`, folding each interleaved callback buffer
/// to mono `f32` and forwarding it to the worker over `tx`.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: mpsc::Sender<Vec<f32>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let chans = channels.max(1);
    device
        .build_input_stream(
            *config, // cpal 0.18 takes the config by value (StreamConfig: Copy)
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(chans)
                    .map(|frame| {
                        frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / chans as f32
                    })
                    .collect();
                let _ = tx.send(mono);
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("build audio stream: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_device_classification() {
        // Real hardware and sound servers are NOT pseudo — they must stay ahead of plugins.
        for real in ["HDA Intel PCH, ALC897 Analog", "PipeWire Sound Server", "PulseAudio Sound Server"] {
            assert!(!is_pseudo_device(&real.to_lowercase()), "{real} wrongly flagged pseudo");
        }
        // ALSA plugin/pseudo devices (the ones that opened but delivered silence) ARE pseudo.
        for pseudo in [
            "Plugin using Speex DSP (resample, agc, denoise, echo, dereverb)",
            "Rate Converter Plugin Using Libav/FFmpeg Library",
            "Plugin for channel upmix (4,6,8)",
            "Default ALSA Output (currently PipeWire Media Server)",
            "Discard all samples (playback) or generate zero samples (capture)",
        ] {
            assert!(is_pseudo_device(&pseudo.to_lowercase()), "{pseudo} should be pseudo");
        }
        assert!(is_null_device(&"Discard all samples (playback) or generate zero samples (capture)".to_lowercase()));
    }
}

#[cfg(test)]
mod livetest {
    //! Device-dependent smoke test for the capture wiring — `#[ignore]`d so CI skips it
    //! (the rest of the suite is device-free per repo convention). Run manually:
    //! `cargo test -p yeelight-gui audio::capture::livetest -- --ignored --nocapture`
    use super::*;
    use crate::audio::dsp::NUM_BARS;

    #[test]
    #[ignore = "needs a real audio input device"]
    fn opens_and_publishes() {
        println!("inputs(): {:?}", inputs());
        let (tx, rx) = watch::channel(vec![0.0; NUM_BARS]);
        let (_cfg_tx, cfg_rx) = watch::channel(AudioConfig::default());
        let guard = spawn(None, tx, cfg_rx).expect("spawn failed");
        std::thread::sleep(Duration::from_millis(500));
        let bars = rx.borrow().clone();
        println!("bars = {bars:?}");
        drop(guard);
        assert_eq!(bars.len(), NUM_BARS);
    }
}
