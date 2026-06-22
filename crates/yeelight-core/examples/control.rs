//! Discover a device, connect, drive it, and print pushed notifications.
//!
//! Run: `cargo run -p yeelight-core --example control`

use std::time::Duration;

use tokio::time::sleep;
use yeelight_core::{Client, Effect, discovery, firewall};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    firewall::ensure_udp_open(discovery::SSDP_PORT).await?;
    let device = discovery::search(Duration::from_secs(3))
        .await?
        .into_iter()
        .next()
        .ok_or("no device found")?;
    println!("connecting to {} @ {}", device.id, device.location);

    let client = Client::connect(device).await?;

    let mut notes = client.notifications();
    tokio::spawn(async move {
        while let Ok(n) = notes.recv().await {
            println!("notify: {:?}", n.params);
        }
    });

    client.set_power(true, Effect::Smooth(500), None).await?;
    client.set_rgb(0xFF0000, Effect::Smooth(500)).await?;
    sleep(Duration::from_millis(800)).await;

    let props = client.get_prop(&["power", "bright", "rgb"]).await?;
    println!("props (power, bright, rgb): {props:?}");

    sleep(Duration::from_secs(1)).await;
    Ok(())
}
