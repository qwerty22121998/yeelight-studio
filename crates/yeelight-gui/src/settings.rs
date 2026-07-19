//! Persist the GUI's user settings to `~/.config/yeelight-studio/settings.toml`.
//!
//! Discovered devices are cached separately in the same directory
//! (`devices.toml`) via [`yeelight_core::registry`]; [`devices_path`] points at it.
//! All I/O here is best-effort: read failures fall back to defaults and write
//! failures are logged, never fatal — settings persistence must not crash the UI.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::message::ThemePref;

/// User-tunable settings that survive across launches (the Settings screen).
pub(crate) struct Settings {
    /// Ignore device `support` sets and show/send every control.
    pub(crate) force_all: bool,
    /// Discovery scan timeout in seconds.
    pub(crate) timeout_secs: u32,
    /// Theme preference.
    pub(crate) theme_pref: ThemePref,
}

impl Default for Settings {
    /// Matches [`crate::app::App`]'s defaults so a missing file behaves like a
    /// fresh install.
    fn default() -> Self {
        Self { force_all: false, timeout_secs: 3, theme_pref: crate::theme::default_pref() }
    }
}

/// On-disk form. `iced::Theme` doesn't implement `Serialize`, so the theme is
/// stored as its `Display` string and matched back in [`theme_pref_from_str`].
#[derive(Serialize, Deserialize)]
struct Stored {
    force_all: bool,
    timeout_secs: u32,
    theme: String,
}

/// Per-user config directory for `yeelight-studio`, via the `dirs` crate:
/// `%APPDATA%` (Windows), `~/Library/Application Support` (macOS),
/// `$XDG_CONFIG_HOME` or `~/.config` (Linux). Falls back to `.` if none resolves.
fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yeelight-studio")
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Path to the cached device registry (consumed by [`yeelight_core::registry`]).
pub(crate) fn devices_path() -> PathBuf {
    config_dir().join("devices.toml")
}

/// Path to the human-readable command log (appended by the logging screen).
pub(crate) fn log_path() -> PathBuf {
    config_dir().join("commands.log")
}

/// Resolve a stored theme string back to a [`ThemePref`]. Unknown strings (e.g. a
/// theme removed from a newer iced) fall back to the default.
fn theme_pref_from_str(s: &str) -> ThemePref {
    if s == crate::theme::EMBER_NAME {
        return crate::theme::default_pref();
    }
    if s == ThemePref::System.to_string() {
        return ThemePref::System;
    }
    iced::Theme::ALL
        .iter()
        .find(|t| t.to_string() == s)
        .map(|t| ThemePref::Fixed(t.clone()))
        .unwrap_or_else(crate::theme::default_pref)
}

/// Load settings, falling back to defaults on a missing or unreadable file.
pub(crate) fn load() -> Settings {
    let stored: Stored = match std::fs::read_to_string(settings_path()) {
        Ok(text) => match toml::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "settings: parse failed, using defaults");
                return Settings::default();
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            tracing::warn!(error = %e, "settings: read failed, using defaults");
            return Settings::default();
        }
    };
    Settings {
        force_all: stored.force_all,
        timeout_secs: stored.timeout_secs,
        theme_pref: theme_pref_from_str(&stored.theme),
    }
}

/// Persist settings. Best-effort: any failure is logged and swallowed.
pub(crate) fn save(s: &Settings) {
    let dir = config_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "settings: mkdir failed, not saved");
        return;
    }
    let stored = Stored {
        force_all: s.force_all,
        timeout_secs: s.timeout_secs,
        theme: s.theme_pref.to_string(),
    };
    match toml::to_string_pretty(&stored) {
        Ok(text) => {
            if let Err(e) = std::fs::write(settings_path(), text) {
                tracing::warn!(error = %e, "settings: write failed");
            }
        }
        Err(e) => tracing::warn!(error = %e, "settings: serialize failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_pref_round_trips() {
        assert_eq!(theme_pref_from_str(&ThemePref::System.to_string()), ThemePref::System);
        assert_eq!(theme_pref_from_str(crate::theme::EMBER_NAME), crate::theme::default_pref());
        for t in iced::Theme::ALL {
            let pref = ThemePref::Fixed(t.clone());
            assert_eq!(theme_pref_from_str(&pref.to_string()), pref);
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        assert_eq!(theme_pref_from_str("NoSuchTheme"), crate::theme::default_pref());
    }
}
