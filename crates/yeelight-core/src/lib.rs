//! Discovery and local control of Yeelight WiFi LEDs over the LAN.
//!
//! Implements the [Yeelight inter-operation spec](../../../docs/yeelight-spec.md):
//! SSDP-like [`discovery`] on multicast `239.255.255.250:1982`, a JSON-over-TCP
//! control [`Client`] with typed [`commands`], asynchronous notifications, and
//! [`music`] mode. The [`firewall`] helpers open the required static ports via `ufw`.
//!
//! ```no_run
//! use std::time::Duration;
//! use yeelight_core::{discovery, firewall, Client, Effect};
//!
//! # async fn run() -> yeelight_core::Result<()> {
//! firewall::ensure_udp_open(discovery::SSDP_PORT).await?;
//! let devices = discovery::search(Duration::from_secs(3)).await?;
//! if let Some(device) = devices.into_iter().next() {
//!     let client = Client::connect(device).await?;
//!     client.set_power(true, Effect::Smooth(500), None).await?;
//!     client.set_rgb(0xFF0000, Effect::Smooth(500)).await?;
//! }
//! # Ok(())
//! # }
//! ```
#![deny(missing_docs)]

pub mod client;
pub mod commands;
pub mod device;
pub mod discovery;
pub mod error;
pub mod firewall;
pub mod message;
pub mod music;
pub mod registry;

pub use client::{Client, Direction, LogLine};
pub use commands::{
    AdjustAction, AdjustProp, CronType, Effect, FlowAction, FlowExpr, FlowTuple, PowerMode, Scene,
};
pub use device::{Device, Model, State};
pub use discovery::{Listener, search};
pub use error::{Error, Result};
pub use message::Notification;
pub use music::{DEFAULT_MUSIC_PORT, MusicConnection};
