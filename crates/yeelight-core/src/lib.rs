//! Discovery and local control of Yeelight WiFi LEDs over the LAN.
//!
//! Implements the [Yeelight inter-operation spec](../../../docs/yeelight-spec.md).
#![deny(missing_docs)]

pub mod client;
pub mod commands;
pub mod device;
pub mod discovery;
pub mod error;
pub mod firewall;
pub mod message;
pub mod music;

pub use client::Client;
pub use commands::{
    AdjustAction, AdjustProp, CronType, Effect, FlowAction, FlowExpr, FlowTuple, PowerMode, Scene,
};
pub use device::{Device, Model, State};
pub use discovery::{Listener, search};
pub use error::{Error, Result};
pub use message::Notification;
pub use music::{DEFAULT_MUSIC_PORT, MusicConnection};
