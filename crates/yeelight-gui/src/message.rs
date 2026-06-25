//! UI messages and the small enums that travel inside them.

use std::collections::HashMap;
use std::sync::Arc;

use iced::Color;
use tokio::sync::Mutex;
use yeelight_core::{Client, Device, Direction, MusicConnection};

/// A shared music channel: `Arc<Mutex<..>>` so it can be cloned into async
/// streaming tasks while still living in [`crate::app::App`] state.
/// `MusicConnection::send` takes `&mut self`, so the lock is required.
pub(crate) type MusicSession = Arc<Mutex<MusicConnection>>;

/// What the detail pane shows: the selected device, or the settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Screen {
    /// Controls for the selected device.
    #[default]
    Device,
    /// The settings screen.
    Settings,
    /// The command-log screen.
    Logging,
}

/// Which tab of the settings pane is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsTab {
    /// Discovery + control behaviour.
    #[default]
    General,
    /// Theme selection.
    Appearance,
}

/// Theme preference: one of iced's built-in themes, or follow the OS.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum ThemePref {
    /// Follow the OS light/dark setting (resolved when selected).
    #[default]
    System,
    /// A specific built-in [`iced::Theme`] (one of [`iced::Theme::ALL`]).
    Fixed(iced::Theme),
}

impl std::fmt::Display for ThemePref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemePref::System => f.write_str("Respect system"),
            ThemePref::Fixed(theme) => theme.fmt(f),
        }
    }
}

/// Which control tab of the detail pane is shown for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DetailTab {
    /// Color/temperature controls — the light is in one mode at a time, picked
    /// by a Color|Temperature segment.
    #[default]
    Light,
    /// Preset scenes.
    Scenes,
    /// Color-flow presets + custom editor.
    Flow,
    /// Sleep timer.
    Timer,
    /// Music "instant control" mode.
    Music,
    /// Ambient screen-capture mode.
    // Constructed by the Ambient tab view (a later task), not in this chunk yet.
    #[allow(dead_code)]
    Ambient,
}

/// Which editable field of a custom flow-editor row changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlowField {
    /// Duration in ms.
    Duration,
    /// Mode: color / temperature / sleep.
    Mode,
    /// RGB or CT value.
    Value,
    /// Brightness.
    Bright,
}

/// A user intent issued from a control widget, before device capabilities resolve it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CmdKind {
    /// Flip power.
    Toggle,
    /// Set RGB color.
    SetColor(Color),
    /// Set brightness `1..=100`.
    SetBright(u8),
    /// Set color temperature in Kelvin (`1700..=6500`).
    SetTemp(u16),
}

/// Which control a command came from — used to disable only the clicked button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Btn {
    /// The toggle button.
    Toggle,
    /// The change-color button.
    Color,
    /// The brightness slider.
    Bright,
    /// The color-temperature slider.
    Temp,
    /// One-shot whole-device actions (rename, save-default, …) that don't gate a
    /// specific control button. Kept distinct so completing one never clears a
    /// real per-button in-flight key.
    Misc,
}

/// Identity of one in-flight control: `(device id, background?, which button)`.
pub(crate) type OpKey = (String, bool, Btn);

/// A fully resolved device operation (capabilities already taken into account).
///
/// The leading `bool` is the background-light flag (`true` = `bg_*`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    /// `toggle` / `bg_toggle`.
    Toggle(bool),
    /// `set_power` / `bg_set_power` with an explicit on/off target.
    SetPower(bool, bool),
    /// `set_rgb` / `bg_set_rgb`.
    SetRgb(bool, u32),
    /// `set_bright` / `bg_set_bright`.
    SetBright(bool, u8),
    /// `set_ct_abx` / `bg_set_ct_abx`.
    SetCt(bool, u16),
}

impl Op {
    /// Short human label for the status bar on success.
    pub(crate) fn label(self) -> String {
        let which = |bg: bool| if bg { "background" } else { "main" };
        match self {
            Op::Toggle(bg) | Op::SetPower(bg, _) => format!("{} light power set", which(bg)),
            Op::SetRgb(bg, rgb) => format!("{} light color set to #{rgb:06X}", which(bg)),
            Op::SetBright(bg, b) => format!("{} light brightness set to {b}%", which(bg)),
            Op::SetCt(bg, ct) => format!("{} light temperature set to {ct}K", which(bg)),
        }
    }
}

/// Every event the application reacts to.
#[derive(Clone)]
pub(crate) enum Message {
    /// Start a LAN scan.
    Scan,
    /// Scan finished (devices or an error string).
    Scanned(Result<Vec<Device>, String>),
    /// Native open-port permission popup was answered (`true` = open it).
    PortPromptAnswered(bool),
    /// The sudo open-port attempt finished.
    PortOpened(Result<(), String>),
    /// Switch the detail pane between the device screen and settings.
    SelectScreen(Screen),
    /// Select a device tab by index.
    SelectTab(usize),
    /// Switch the active control tab for one light of the selected device.
    SelectDetailTab {
        /// Background light if true, else main.
        bg: bool,
        /// The tab to show.
        tab: DetailTab,
    },
    /// Pick the Light tab's segment: `temp` = color-temperature, else color.
    SetLightSeg {
        /// Background light if true, else main.
        bg: bool,
        /// Show the temperature segment if true, else the color segment.
        temp: bool,
    },
    /// Begin editing the selected device's name (seeds the buffer).
    RenameStart,
    /// Edit the in-progress rename buffer.
    RenameEdit(String),
    /// Commit the rename (`set_name`).
    RenameCommit,
    /// Cancel an in-progress rename.
    RenameCancel,
    /// Apply a preset scene by index into `presets::SCENES`.
    ApplyScene {
        /// Background light?
        bg: bool,
        /// Index into `presets::SCENES`.
        index: usize,
    },
    /// Save the current state as the power-on default (`set_default`).
    SaveDefault,
    /// Apply a preset color flow by index into `presets::FLOWS`.
    ApplyFlowPreset {
        /// Background light?
        bg: bool,
        /// Index into `presets::FLOWS`.
        index: usize,
    },
    /// Stop any running color flow (`stop_cf`).
    StopFlow {
        /// Background light?
        bg: bool,
    },
    /// Add an empty row to the custom flow-editor draft.
    FlowRowAdd {
        /// Background light?
        bg: bool,
    },
    /// Remove flow-editor row `row`.
    FlowRowDel {
        /// Background light?
        bg: bool,
        /// Row index.
        row: usize,
    },
    /// Edit a field of flow-editor row `row`.
    FlowRowEdit {
        /// Background light?
        bg: bool,
        /// Row index.
        row: usize,
        /// Which field changed.
        field: FlowField,
        /// New raw string value (parsed on apply).
        value: String,
    },
    /// Change the custom-flow repeat count (raw input string).
    FlowCountEdit {
        /// Background light?
        bg: bool,
        /// Raw count input.
        value: String,
    },
    /// Start the custom flow from the current draft (`start_cf`).
    StartCustomFlow {
        /// Background light?
        bg: bool,
    },
    /// Edit the sleep-timer minutes input (raw string).
    TimerEdit(String),
    /// Start the sleep timer (`cron_add`).
    TimerStart,
    /// Cancel the sleep timer (`cron_del`).
    TimerCancel,
    /// Per-second countdown tick for active timers.
    TimerTick,
    /// Toggle music "instant control" mode for the selected device.
    MusicToggle,
    /// A music session finished starting (or failed).
    MusicStarted {
        /// Device id the session belongs to.
        id: String,
        /// The session handle or an error string.
        session: Result<MusicSession, String>,
    },
    /// Start or stop ambient screen-capture mode for the selected device.
    // The Ambient*-emitting variants below are constructed by the Ambient tab view
    // (a later task); allow dead code until that lands.
    #[allow(dead_code)]
    AmbientToggle,
    /// A resolved ambient sink is ready (music started or fell back to direct), or failed.
    AmbientStarted {
        /// Device id the session belongs to.
        id: String,
        /// The sink to drive the bulb, or an error string.
        sink: Result<crate::ambient::AmbientSink, String>,
        /// Whether ambient started this music session itself (so it stops it on toggle-off).
        own_music: bool,
    },
    /// Change the ambient capture region for the selected device.
    #[allow(dead_code)]
    AmbientSetRegion(crate::ambient::color::Region),
    /// Change the ambient extraction mode for the selected device.
    #[allow(dead_code)]
    AmbientSetMode(crate::ambient::color::ExtractMode),
    /// Toggle an ambient target light (`main` = main light, else background).
    #[allow(dead_code)]
    AmbientSetTarget {
        /// Main light if true, else background.
        main: bool,
        /// Enable that target.
        on: bool,
    },
    /// Change the ambient capture monitor (None = primary). Only while stopped.
    #[allow(dead_code)]
    AmbientSetMonitor(Option<u32>),
    /// An ambient send failed (surfaced in the status bar).
    AmbientError {
        /// Device id.
        id: String,
        /// Error text.
        error: String,
    },
    /// A control was activated for the selected device.
    Command {
        /// Background light?
        bg: bool,
        /// What the control wants.
        kind: CmdKind,
    },
    /// A post-scan connect resolved: cache the client (if it succeeded) so its
    /// notification stream starts. No command to run — listening only.
    Listening {
        /// Device id this connection belongs to.
        id: String,
        /// The connected client or an error (offline devices are skipped).
        client: Result<Arc<Client>, String>,
    },
    /// A lazy connection attempt resolved; run `op` if it succeeded.
    Connected {
        /// The button whose command triggered this connect.
        key: OpKey,
        /// The connected client or an error.
        client: Result<Arc<Client>, String>,
        /// The operation to run on success.
        op: Op,
    },
    /// A device command finished (ok-label or error).
    CommandDone {
        /// The button that issued the command.
        key: OpKey,
        /// Ok-label or error.
        result: Result<String, String>,
    },
    /// Open the color picker overlay.
    OpenPicker {
        /// Background light?
        bg: bool,
    },
    /// Cancel the color picker overlay.
    CancelPicker {
        /// Background light?
        bg: bool,
    },
    /// A color was submitted from the picker.
    PickColor {
        /// Background light?
        bg: bool,
        /// Chosen color.
        color: Color,
    },
    /// Brightness slider dragged (draft only; command sent on release).
    BrightDraft {
        /// Background light?
        bg: bool,
        /// Draft brightness `1..=100`.
        value: u8,
    },
    /// Temperature slider dragged (draft only; command sent on release).
    TempDraft {
        /// Background light?
        bg: bool,
        /// Draft color temperature in Kelvin.
        value: u16,
    },
    /// "Enable all controls" setting toggled (ignore device `support`).
    ForceAllToggled(bool),
    /// Switch the settings sub-tab.
    SelectSettingsTab(SettingsTab),
    /// Theme preference changed.
    ThemeChanged(ThemePref),
    /// Discover-timeout setting changed.
    TimeoutChanged(String),
    /// Periodic tick that advances the scan progress bar.
    Tick,
    /// A live `props` notification arrived for a connected device.
    StateChanged {
        /// Device id.
        id: String,
        /// Property name → value (all strings, per spec).
        params: HashMap<String, String>,
    },
    /// A raw wire line was captured (sent or received) for a connected device.
    Logged {
        /// Device id the line belongs to.
        id: String,
        /// Sent or received.
        direction: Direction,
        /// The raw JSON line.
        line: String,
    },
    /// Clear the in-memory command log (the on-disk file is kept).
    LogClear,
    /// Pause or resume command-log capture (screen and file).
    LogTogglePause,
    /// Open the on-disk command log in the desktop's default application.
    LogOpenFile,
    /// Filter the log view to one device id, or `None` to show all.
    LogFilterDevice(Option<String>),
}
