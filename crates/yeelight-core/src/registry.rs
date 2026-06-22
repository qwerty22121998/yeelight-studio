//! Persist discovered devices to a TOML file so later runs can skip a fresh LAN search.
//!
//! Devices are stored as a `[[device]]` array of tables, keyed by their discovery `id`.
//!
//! ```no_run
//! use yeelight_core::registry;
//!
//! # async fn run() -> yeelight_core::Result<()> {
//! let devices = yeelight_core::search(std::time::Duration::from_secs(3)).await?;
//! registry::save("yeelight-devices.toml", &devices)?;
//! // ... next run:
//! let known = registry::load("yeelight-devices.toml")?;
//! # Ok(())
//! # }
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::device::Device;
use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default, rename = "device")]
    devices: Vec<Device>,
}

/// Write `devices` to `path` as TOML, replacing any existing file.
pub fn save(path: impl AsRef<Path>, devices: &[Device]) -> Result<()> {
    let reg = Registry {
        devices: devices.to_vec(),
    };
    std::fs::write(path, toml::to_string_pretty(&reg)?)?;
    Ok(())
}

/// Load devices previously written with [`save`]. A missing file yields an empty list.
pub fn load(path: impl AsRef<Path>) -> Result<Vec<Device>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(toml::from_str::<Registry>(&text)?.devices),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Model, State};

    fn sample() -> Device {
        Device {
            id: "0x000000000015243f".into(),
            model: Model::Color,
            fw_ver: "18".into(),
            location: "192.168.1.239:55443".parse().unwrap(),
            support: ["get_prop", "set_power"].iter().map(|s| s.to_string()).collect(),
            state: State {
                power: Some(true),
                bright: Some(100),
                ..Default::default()
            },
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let reg = Registry {
            devices: vec![sample()],
        };
        let text = toml::to_string_pretty(&reg).expect("serialize");
        let back: Registry = toml::from_str(&text).expect("deserialize");
        let d = &back.devices[0];
        assert_eq!(d.id, "0x000000000015243f");
        assert_eq!(d.model, Model::Color);
        assert_eq!(d.location, "192.168.1.239:55443".parse().unwrap());
        assert_eq!(d.state.bright, Some(100));
        assert!(d.support.contains("set_power"));
    }
}
