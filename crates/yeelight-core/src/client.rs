//! Persistent TCP control connection to a device (spec §4).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::message::{Command, Incoming, Notification, parse_line};

/// A reply waiter: `Ok(result)` on success, `Err((code, message))` on a device error.
type ReplyTx = oneshot::Sender<std::result::Result<Value, (i64, String)>>;
type Pending = Arc<Mutex<HashMap<u64, ReplyTx>>>;

/// A control connection to a single device.
///
/// One connection multiplexes synchronous command/result pairs and the device's
/// asynchronous state notifications. Typed methods live in [`crate::commands`].
///
/// The device enforces its own limits (max 4 connections, 60 commands/min per
/// connection, 144/min LAN-wide); this client does not rate-limit.
pub struct Client {
    device: Device,
    writer: Mutex<OwnedWriteHalf>,
    next_id: AtomicU64,
    pending: Pending,
    notifications: broadcast::Sender<Notification>,
    local_addr: SocketAddr,
    /// When set, [`Client::call_supported`] skips the local `support`-set check.
    force: AtomicBool,
}

impl Client {
    /// Per-request timeout for [`Client::call`].
    pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// Connect to `device` over TCP using the port from its discovery `Location`.
    ///
    /// Spawns a background task that reads `\r\n`-delimited lines, routing results to
    /// the matching [`Client::call`] waiter and notifications to [`Client::notifications`].
    pub async fn connect(device: Device) -> Result<Self> {
        let stream = TcpStream::connect(device.location).await?;
        let local_addr = stream.local_addr()?;
        let (read, write) = stream.into_split();

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, _rx) = broadcast::channel(64);

        let pending_r = pending.clone();
        let tx_r = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                tracing::trace!(%line, "recv");
                match parse_line(&line) {
                    Ok(Incoming::Result { id, result }) => {
                        if let Some(s) = pending_r.lock().await.remove(&id) {
                            let _ = s.send(Ok(result));
                        }
                    }
                    Ok(Incoming::Error { id, code, message }) => {
                        if let Some(s) = pending_r.lock().await.remove(&id) {
                            let _ = s.send(Err((code, message)));
                        }
                    }
                    Ok(Incoming::Notification(n)) => {
                        let _ = tx_r.send(n);
                    }
                    Err(e) => tracing::debug!("ignoring unparseable line: {e}"),
                }
            }
            tracing::info!("control connection closed");
            // Dropping `pending_r` here drops every waiting sender; their receivers
            // resolve to `RecvError`, which `call` maps to `Error::NotConnected`.
        });

        Ok(Self {
            device,
            writer: Mutex::new(write),
            next_id: AtomicU64::new(1),
            pending,
            notifications: tx,
            local_addr,
            force: AtomicBool::new(false),
        })
    }

    /// Send a raw command and await its result (the generic escape hatch).
    ///
    /// Prefer the typed methods in [`crate::commands`]; use this for methods this
    /// crate does not wrap.
    pub async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cmd = Command {
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&cmd)?;
        line.push_str("\r\n");

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        tracing::debug!(id, method, "sending command");
        tracing::trace!(line = %line.trim_end(), "send");
        {
            let mut w = self.writer.lock().await;
            w.write_all(line.as_bytes()).await?;
            w.flush().await?;
        }

        let result = match tokio::time::timeout(Self::REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err((code, message)))) => Err(Error::Protocol { code, message }),
            Ok(Err(_)) => Err(Error::NotConnected), // sender dropped: connection closed
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(Error::Timeout)
            }
        };
        if let Err(e) = &result {
            tracing::warn!(id, method, error = %e, "command failed");
        }
        result
    }

    /// Like [`Client::call`] but first verifies the device advertises `method`,
    /// unless [`Client::set_force`] has disabled that check.
    pub(crate) async fn call_supported(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        if !self.force.load(Ordering::Relaxed) && !self.device.supports(method) {
            return Err(Error::Unsupported(method.to_string()));
        }
        self.call(method, params).await
    }

    /// Send commands even when the device's `support` set omits the method.
    ///
    /// Off by default. When on, the typed methods no longer fail fast with
    /// [`Error::Unsupported`]; the device itself may still reject the command
    /// with [`Error::Protocol`]. Useful for bulbs that under-report support.
    pub fn set_force(&self, force: bool) {
        self.force.store(force, Ordering::Relaxed);
    }

    /// Subscribe to the stream of state-change notifications (spec §4.3).
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    /// The device this client controls.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Local socket address of the control connection (the LAN IP for music mode).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}
