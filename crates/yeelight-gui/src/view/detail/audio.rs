//! Music-capture section: pick an audio input + mapping mode + target light(s), watch the
//! cava-style wave, then stream a music-reactive color to the bulb (music mode if available,
//! else rate-limited `set_rgb`). Device-wide — its own detail-pane section, not a per-light
//! tab, because one audio capture drives the main and/or background light.

use iced::widget::{button, checkbox, column, container, pick_list, row, slider, text};
use iced::{Background, Border, Color, Element, Length::Fill, Theme};
use yeelight_core::Device;

use super::color_modes;
use crate::ambient::color::Rgb;
use crate::app::App;
use crate::audio::dsp::MusicMode;
use crate::message::{AudioTab, Message};
use crate::view::components::{chip, hex, swatch, tab_strip};
use crate::view::wave::Wave;

/// Render the Music-capture section body (device-wide; main and/or background targets).
pub(crate) fn body<'a>(app: &'a App, d: &'a Device) -> Element<'a, Message> {
    let running = app.audio.contains_key(&d.id);
    let cfg = app.audio_cfg.get(&d.id).cloned().unwrap_or_default();

    // Mode as a segmented control (like the preview and the app's Color|Temperature segment)
    // rather than a dropdown — four fixed modes read better as always-visible chips.
    let mut mode = row![].spacing(4);
    for m in MusicMode::ALL {
        mode = mode.push(chip(mode_label(m), m == cfg.mode, Message::AudioSetMode(m)));
    }

    // Target checkboxes — each shown if that light has any color control (same as ambient).
    let (main_rgb, main_ct) = color_modes(app, d, false);
    let (bg_rgb, bg_ct) = color_modes(app, d, true);
    let mut targets = column![].spacing(6);
    if main_rgb || main_ct {
        let label = if main_rgb { "Main light" } else { "Main light (temperature)" };
        targets = targets.push(
            checkbox(cfg.main)
                .label(label)
                .on_toggle(|on| Message::AudioSetTarget { main: true, on }),
        );
    }
    if bg_rgb || bg_ct {
        let label = if bg_rgb { "Background light" } else { "Background light (temperature)" };
        targets = targets.push(
            checkbox(cfg.bg)
                .label(label)
                .on_toggle(|on| Message::AudioSetTarget { main: false, on }),
        );
    }

    let smooth = checkbox(cfg.smooth)
        .label("Smooth transitions")
        .on_toggle(Message::AudioSetSmooth);

    // "Capture" sub-tab: device / color-mapping mode / targets / smoothing.
    let mut capture_body = column![
        row![text("Mode").width(90), mode].spacing(10).align_y(iced::Center),
        targets,
        smooth,
    ]
    .spacing(12);

    // Input picker: only while stopped (the capture device is fixed at start). Selecting one
    // sets the start-time input. The list is cached in App (enumerating hits the audio server)
    // and refreshed on scan.
    if !running {
        if app.audio_inputs.is_empty() {
            capture_body = capture_body
                .push(text("No audio input devices found.").size(12).color(crate::theme::muted()));
        } else {
            let mut choices = vec![InputChoice::Auto];
            choices.extend(app.audio_inputs.iter().cloned().map(InputChoice::Device));
            let selected = match &cfg.input {
                Some(name) => app
                    .audio_inputs
                    .iter()
                    .find(|n| *n == name)
                    .cloned()
                    .map_or(InputChoice::Auto, InputChoice::Device),
                None => InputChoice::Auto,
            };
            capture_body = capture_body.push(
                row![
                    text("Input").width(90),
                    pick_list(choices, Some(selected), |c| match c {
                        InputChoice::Auto => Message::AudioSetInput(None),
                        InputChoice::Device(n) => Message::AudioSetInput(Some(n)),
                    })
                    .style(crate::theme::pick_list),
                ]
                .spacing(10)
                .align_y(iced::Center),
            );
            // The default input captures the sink monitor (whatever's playing), not the mic.
            capture_body = capture_body.push(
                text("Default input reacts to system audio (whatever\u{2019}s playing).")
                    .size(11)
                    .color(crate::theme::muted()),
            );
        }
    }

    // Sub-tab strip + the active body. Capture = the controls above; Tune = live DSP knobs.
    let tabs = tab_strip(
        &[("Capture", AudioTab::Capture), ("Tune", AudioTab::Tune)],
        app.audio_tab,
        Message::SelectAudioTab,
    );
    let controls: Element<'a, Message> = match app.audio_tab {
        AudioTab::Capture => capture_body.into(),
        AudioTab::Tune => tune(cfg.gain, cfg.decay, cfg.bars),
    };

    // Left rail: the live bulb color over a caption — the section's compact status column,
    // same rail-plus-caption shape as the light sections' brightness dial.
    let color = app.audio_color.get(&d.id).copied().unwrap_or(Rgb::BLACK);
    let sw = Color::from_rgb8(color.r, color.g, color.b);
    let rail = column![
        swatch(sw, 44.0),
        text(hex(sw)).size(10).color(crate::theme::muted()),
        text("Bulb").size(11).color(crate::theme::muted()),
    ]
    .spacing(6)
    .align_x(iced::Center);

    // The cava wave, the section's visual anchor, next to the bulb rail. Live bars while
    // running; a gentle idle ripple (animated via `audio_phase`) while stopped — so the
    // visualizer is always alive, like the preview. Both use the configured bar count.
    let bars: Vec<f32> = if running {
        app.audio_bars.get(&d.id).cloned().unwrap_or_else(|| vec![0.0; cfg.bars])
    } else {
        crate::audio::dsp::idle_bars(app.audio_phase, cfg.bars)
    };
    let wave = container(Wave::new(bars)).padding(8).width(Fill).style(wave_box);

    // The section title lives in the collapsible header (see `detail::collapsible`).
    let label = if running { "Stop music capture" } else { "Start music capture" };
    column![
        status_line(app, d, running),
        row![rail, wave].spacing(16),
        tabs,
        controls,
        button(text(label))
            .style(crate::theme::primary_button)
            .on_press(Message::AudioToggle),
    ]
    .spacing(12)
    .into()
}

/// The "Tune" sub-tab body: three live sliders shaping the spectrum — Sensitivity (gain),
/// Smoothing (bar decay) and Bars (band count). Each edits the config live, so a running
/// capture reshapes in real time. These are the cava knobs this app's DSP actually honors.
fn tune<'a>(gain: f32, decay: f32, bars: usize) -> Element<'a, Message> {
    let (bmin, bmax) =
        (*crate::audio::dsp::BARS_RANGE.start() as f32, *crate::audio::dsp::BARS_RANGE.end() as f32);
    column![
        tune_row(
            "Sensitivity",
            slider(1.0..=24.0, gain, Message::AudioSetGain).step(0.5f32).into(),
            format!("{gain:.1}"),
        ),
        tune_row(
            "Smoothing",
            slider(0.50..=0.95, decay, Message::AudioSetSmoothing).step(0.01f32).into(),
            format!("{decay:.2}"),
        ),
        tune_row(
            "Bars",
            slider(bmin..=bmax, bars as f32, |v| Message::AudioSetBars(v.round() as usize))
                .step(1.0f32)
                .into(),
            bars.to_string(),
        ),
    ]
    .spacing(14)
    .into()
}

/// One labeled slider row: fixed-width caption, the slider filling the middle, current value
/// pinned right.
fn tune_row<'a>(
    label: &'a str,
    control: Element<'a, Message>,
    value: String,
) -> Element<'a, Message> {
    row![
        text(label).width(90),
        control,
        text(value).size(12).width(40).color(crate::theme::muted()),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

/// An input-picker choice: the auto/default device (`input = None`) or a specific device.
/// Lets the picker show and re-select the default instead of only concrete devices.
#[derive(Clone, PartialEq)]
enum InputChoice {
    Auto,
    Device(String),
}

impl std::fmt::Display for InputChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputChoice::Auto => f.write_str("Default input (auto)"),
            InputChoice::Device(n) => f.write_str(n),
        }
    }
}

/// Short label for a music mode's segment chip.
fn mode_label(m: MusicMode) -> &'static str {
    match m {
        MusicMode::Spectrum => "Spectrum",
        MusicMode::Pulse => "Pulse",
        MusicMode::Rainbow => "Rainbow",
        MusicMode::Vu => "VU",
    }
}

/// The wave's recessed panel: a slightly darker surface + muted 1px frame.
fn wave_box(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(crate::theme::darken(p.background.base.color, 0.03))),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: crate::theme::RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// Running (with the live send mode) or off.
fn status_line<'a>(app: &App, d: &Device, running: bool) -> Element<'a, Message> {
    if running {
        let on_music = app.audio.get(&d.id).map(|r| r.sink.is_music()).unwrap_or(false);
        let mode = if on_music { "music \u{b7} 30fps" } else { "fallback \u{b7} 2fps" };
        text(format!("Running ({mode}). Light reacts to audio."))
            .color(crate::theme::success())
            .into()
    } else {
        text("Off. Start to react the light to whatever\u{2019}s playing.")
            .color(crate::theme::muted())
            .into()
    }
}
