//! UI messages and the small enums that travel inside them.

use std::collections::HashMap;
use std::sync::Arc;

use iced::Color;
use yeelight_core::{Client, Device};

/// Which left-sidebar pane is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Sidebar {
    /// Device tabs + controls.
    #[default]
    Device,
    /// Settings pane.
    Setting,
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
    /// Startup firewall check: is the discovery port already open?
    PortChecked(Result<bool, String>),
    /// Native open-port permission popup was answered (`true` = open it).
    PortPromptAnswered(bool),
    /// The sudo open-port attempt finished.
    PortOpened(Result<(), String>),
    /// Quit the application.
    Quit,
    /// Switch the sidebar pane.
    SelectSidebar(Sidebar),
    /// Select a device tab by index.
    SelectTab(usize),
    /// A control was activated for the selected device.
    Command {
        /// Background light?
        bg: bool,
        /// What the control wants.
        kind: CmdKind,
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
}
