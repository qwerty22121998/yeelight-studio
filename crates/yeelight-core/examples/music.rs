//! Enter music mode and cycle colors with no rate limit.
//!
//! Run: `cargo run -p yeelight-core --example music`

use std::time::Duration;

use serde_json::json;
use tokio::time::sleep;
use yeelight_core::{Client, DEFAULT_MUSIC_PORT, Effect, MusicConnection, discovery, firewall};

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

    let client = Client::connect(device).await?;
    client.set_power(true, Effect::Smooth(300), None).await?;

    let mut music = MusicConnection::start(&client, DEFAULT_MUSIC_PORT).await?;
    println!("music mode on; cycling colors");
    for rgb in [0xFF0000u32, 0x00FF00, 0x0000FF, 0xFFFF00] {
        music
            .send("set_rgb", vec![json!(rgb), json!("smooth"), json!(300)])
            .await?;
        sleep(Duration::from_millis(400)).await;
    }

    music.stop(&client).await?;
    println!("music mode off");
    Ok(())
}
