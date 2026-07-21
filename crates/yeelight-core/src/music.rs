//! Music mode (spec §4 `set_music`).
//!
//! The controller starts its own TCP server, tells the device to connect back via
//! `set_music`, and then streams commands over that channel. Under music mode the
//! device reports no properties and enforces no command quota.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::firewall;
use crate::message::Command;

/// Default static TCP port the music server listens on.
pub const DEFAULT_MUSIC_PORT: u16 = 54321;

/// An established music-mode channel.
///
/// Commands sent here are fire-and-forget (the device does not reply in music mode)
/// and bypass the normal rate limit.
pub struct MusicConnection {
    stream: TcpStream,
    next_id: u64,
}

impl MusicConnection {
    /// Start music mode against `client`, listening on `port`.
    ///
    /// Opens the firewall for `port`, binds a listener, enables music mode on the
    /// device with our LAN IP, and accepts the device's connect-back (5 s timeout).
    pub async fn start(client: &Client, port: u16) -> Result<Self> {
        firewall::ensure_tcp_open(port).await?;
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;

        let host = client.local_addr().ip();
        tracing::info!("enabling music mode; device will connect back to {host}:{port}");
        client.set_music_on(host, port).await?;

        let (stream, peer) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .map_err(|_| Error::Timeout)??;
        tracing::info!("music channel established from {peer}");

        Ok(Self { stream, next_id: 1 })
    }

    /// Send a command over the music channel (unlimited, no quota, no reply).
    pub async fn send(&mut self, method: &str, params: Vec<Value>) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let cmd = Command {
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&cmd)?;
        line.push_str("\r\n");
        self.stream.write_all(line.as_bytes()).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Disable music mode on the device and close the channel.
    pub async fn stop(mut self, client: &Client) -> Result<()> {
        let _ = self.stream.shutdown().await;
        client.set_music_off().await
    }
}
