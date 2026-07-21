//! Discovered device model and parsed state (spec §3.1).

use std::collections::HashSet;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Yeelight product model (the `model` discovery header).
///
/// Serialized as the raw `model` string (e.g. `"color"`) so persisted registries stay readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Model {
    /// Brightness only.
    Mono,
    /// Color + color temperature.
    Color,
    /// LED stripe.
    Stripe,
    /// Ceiling light.
    Ceiling,
    /// Bedside lamp.
    BsLamp,
    /// Any model not known to this crate.
    Unknown(String),
}

impl From<&str> for Model {
    fn from(s: &str) -> Self {
        match s {
            "mono" => Model::Mono,
            "color" => Model::Color,
            "stripe" => Model::Stripe,
            "ceiling" => Model::Ceiling,
            "bslamp" => Model::BsLamp,
            other => Model::Unknown(other.to_string()),
        }
    }
}

impl From<String> for Model {
    fn from(s: String) -> Self {
        Model::from(s.as_str())
    }
}

impl From<Model> for String {
    fn from(m: Model) -> Self {
        match m {
            Model::Mono => "mono".into(),
            Model::Color => "color".into(),
            Model::Stripe => "stripe".into(),
            Model::Ceiling => "ceiling".into(),
            Model::BsLamp => "bslamp".into(),
            Model::Unknown(s) => s,
        }
    }
}

/// Snapshot of device state parsed from discovery headers or notifications.
///
/// Fields are `Option` because validity is mode-dependent (e.g. `rgb` only in color mode).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Power: `Some(true)` = on, `Some(false)` = off.
    pub power: Option<bool>,
    /// Brightness percentage, `1..=100`.
    pub bright: Option<u8>,
    /// Color mode: `1` rgb, `2` color-temperature, `3` hsv.
    pub color_mode: Option<u8>,
    /// Color temperature in Kelvin (valid when `color_mode == 2`).
    pub ct: Option<u16>,
    /// RGB value (valid when `color_mode == 1`).
    pub rgb: Option<u32>,
    /// Hue `0..=359` (valid when `color_mode == 3`).
    pub hue: Option<u16>,
    /// Saturation `0..=100` (valid when `color_mode == 3`).
    pub sat: Option<u8>,
    /// Device name set via `set_name`.
    pub name: Option<String>,
    /// Background-light power (`bg_power`), for devices with a second light.
    pub bg_power: Option<bool>,
    /// Background-light brightness (`bg_bright`).
    pub bg_bright: Option<u8>,
    /// Background-light color mode (`bg_lmode`): `1` rgb, `2` ct, `3` hsv.
    pub bg_color_mode: Option<u8>,
    /// Background-light color temperature (`bg_ct`).
    pub bg_ct: Option<u16>,
    /// Background-light RGB (`bg_rgb`).
    pub bg_rgb: Option<u32>,
    /// Background-light hue (`bg_hue`).
    pub bg_hue: Option<u16>,
    /// Background-light saturation (`bg_sat`).
    pub bg_sat: Option<u8>,
}

/// A discovered Yeelight device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// Unique device id (the `id` header), used to identify a device.
    pub id: String,
    /// Product model.
    pub model: Model,
    /// Firmware version.
    pub fw_ver: String,
    /// Control-service address parsed from the `Location` header (`yeelight://host:port`).
    pub location: SocketAddr,
    /// Methods the device accepts (the whitespace-separated `support` header).
    pub support: HashSet<String>,
    /// Last known state from discovery.
    pub state: State,
}

impl Device {
    /// Whether the device advertises support for `method`.
    pub fn supports(&self, method: &str) -> bool {
        self.support.contains(method)
    }
}
