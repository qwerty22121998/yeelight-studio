//! Persist the GUI's user settings to `~/.config/yeelight-studio/settings.toml`.
//!
//! Discovered devices are cached separately in the same directory
//! (`devices.toml`) via [`yeelight_core::registry`]; [`devices_path`] points at it.
//! All I/O here is best-effort: read failures fall back to defaults and write
//! failures are logged, never fatal — settings persistence must not crash the UI.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ambient::AmbientConfig;
use crate::audio::AudioConfig;
use crate::message::ThemePref;

/// User-tunable settings that survive across launches (the Settings screen).
pub(crate) struct Settings {
    /// Ignore device `support` sets and show/send every control.
    pub(crate) force_all: bool,
    /// Discovery scan timeout in seconds.
    pub(crate) timeout_secs: u32,
    /// Theme preference.
    pub(crate) theme_pref: ThemePref,
    /// Per-device ambient selections (region/mode/monitor/targets), keyed by device id.
    pub(crate) ambient_cfg: HashMap<String, AmbientConfig>,
    /// Per-device music-capture selections (input/mode/targets), keyed by device id.
    pub(crate) audio_cfg: HashMap<String, AudioConfig>,
}

impl Default for Settings {
    /// Matches [`crate::app::App`]'s defaults so a missing file behaves like a
    /// fresh install.
    fn default() -> Self {
        Self {
            force_all: false,
            timeout_secs: 3,
            theme_pref: crate::theme::default_pref(),
            ambient_cfg: HashMap::new(),
            audio_cfg: HashMap::new(),
        }
    }
}

/// On-disk form. `iced::Theme` doesn't implement `Serialize`, so the theme is
/// stored as its `Display` string and matched back in [`theme_pref_from_str`].
#[derive(Serialize, Deserialize)]
struct Stored {
    force_all: bool,
    timeout_secs: u32,
    theme: String,
    // Map-of-structs serialize as `[table]` sections, so these must stay after the scalar
    // keys (TOML requires it). `default` keeps files written before each field loadable.
    #[serde(default)]
    ambient: HashMap<String, AmbientConfig>,
    #[serde(default)]
    audio: HashMap<String, AudioConfig>,
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
        ambient_cfg: stored.ambient,
        audio_cfg: stored.audio,
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
        ambient: s.ambient_cfg.clone(),
        audio: s.audio_cfg.clone(),
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

    #[test]
    fn ambient_cfg_survives_toml_round_trip() {
        use crate::ambient::color::{ExtractMode, Region};
        let mut ambient = HashMap::new();
        ambient.insert(
            "0x0000000012345678".to_string(),
            AmbientConfig {
                region: Region::Left,
                mode: ExtractMode::Dominant,
                monitor_id: Some(2),
                main: false,
                bg: true,
                smooth: false,
            },
        );
        // Proves scalar keys serialize before the `[ambient.*]` tables (TOML requires it).
        let mut audio = HashMap::new();
        audio.insert(
            "0x0000000012345678".to_string(),
            AudioConfig {
                input: Some("Monitor of Sink".into()),
                mode: crate::audio::dsp::MusicMode::Rainbow,
                main: true,
                bg: false,
                smooth: false,
                ..Default::default()
            },
        );
        let text = toml::to_string_pretty(&Stored {
            force_all: true,
            timeout_secs: 5,
            theme: "System".into(),
            ambient,
            audio,
        })
        .unwrap();
        let back: Stored = toml::from_str(&text).unwrap();
        let cfg = &back.ambient["0x0000000012345678"];
        assert_eq!(cfg.region, Region::Left);
        assert_eq!(cfg.mode, ExtractMode::Dominant);
        assert_eq!(cfg.monitor_id, Some(2));
        assert!(cfg.bg && !cfg.main && !cfg.smooth);
        let acfg = &back.audio["0x0000000012345678"];
        assert_eq!(acfg.input.as_deref(), Some("Monitor of Sink"));
        assert_eq!(acfg.mode, crate::audio::dsp::MusicMode::Rainbow);
        assert!(acfg.main && !acfg.smooth);
    }

    #[test]
    fn pre_ambient_file_still_loads() {
        // A settings.toml written before ambient persistence has no `[ambient]` section.
        let back: Stored =
            toml::from_str("force_all = false\ntimeout_secs = 3\ntheme = \"System\"\n").unwrap();
        assert!(back.ambient.is_empty());
    }
}
