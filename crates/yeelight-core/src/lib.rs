//! Discovery and local control of Yeelight WiFi LEDs over the LAN.
//!
//! Implements the [Yeelight inter-operation spec](../../../docs/yeelight-spec.md).
#![deny(missing_docs)]

pub mod error;

pub use error::{Error, Result};
