//! Discovery and local control of Yeelight WiFi LEDs over the LAN.
//!
//! Implements the [Yeelight inter-operation spec](../../../docs/yeelight-spec.md).
#![deny(missing_docs)]

pub mod device;
pub mod discovery;
pub mod error;
pub mod message;

pub use device::{Device, Model, State};
pub use discovery::{Listener, search};
pub use error::{Error, Result};
pub use message::Notification;
