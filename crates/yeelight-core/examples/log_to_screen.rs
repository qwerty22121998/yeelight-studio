//! Forward library logs to an in-app "screen" (a channel) instead of stderr or a file.
//!
//! `yeelight-core` only *emits* `tracing` events — the consumer picks the sink. Here a
//! custom `MakeWriter` ships each formatted line into a `tokio::broadcast` channel that a
//! "screen" task drains. Swap that task for a TUI log panel and nothing else changes.
//!
//! Run: `cargo run -p yeelight-core --example log_to_screen`

use std::io;

use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;
use yeelight_core::{Client, registry};

/// `MakeWriter` factory: hands the fmt layer a fresh line buffer per event.
#[derive(Clone)]
struct ChannelWriter(broadcast::Sender<String>);

/// Buffers one event's bytes, ships the finished line to the channel on drop.
struct LineBuf {
    tx: broadcast::Sender<String>,
    buf: Vec<u8>,
}

impl io::Write for LineBuf {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineBuf {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let _ = self.tx.send(String::from_utf8_lossy(&self.buf).trim_end().to_owned());
        }
    }
}

impl<'a> MakeWriter<'a> for ChannelWriter {
    type Writer = LineBuf;
    fn make_writer(&'a self) -> LineBuf {
        LineBuf { tx: self.0.clone(), buf: Vec::new() }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, mut rx) = broadcast::channel(256);

    // The "screen": drain log lines and render them. Replace with a TUI log panel.
    let screen = tokio::spawn(async move {
        while let Ok(line) = rx.recv().await {
            println!("[SCREEN] {line}");
        }
    });

    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(ChannelWriter(tx))
        .with_max_level(tracing::Level::TRACE)
        .init();

    let device = registry::load("yeelight-devices.toml")?
        .into_iter()
        .next()
        .ok_or("no saved device")?;
    tracing::info!(id = %device.id, location = %device.location, "using saved device");

    let client = Client::connect(device).await?;
    let props = client.get_prop(&["power", "bright"]).await?;
    tracing::info!(?props, "read device props");

    // Let the screen task flush the last lines before exit.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drop(client);
    screen.abort();
    Ok(())
}
