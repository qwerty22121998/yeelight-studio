//! Discover Yeelight devices on the LAN.
//!
//! Run: `cargo run -p yeelight-core --example discover`

use std::time::Duration;

use yeelight_core::{discovery, firewall, registry};

const REGISTRY: &str = "yeelight-devices.toml";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // YEELIGHT_SUDO=1 escalates the firewall step via `sudo` (prompts for a password).
    if std::env::var("YEELIGHT_SUDO").is_ok() {
        firewall::ensure_udp_open_sudo(discovery::SSDP_PORT).await?;
    } else {
        firewall::ensure_udp_open(discovery::SSDP_PORT).await?;
    }

    let known = registry::load(REGISTRY)?;
    if !known.is_empty() {
        println!("{} device(s) remembered from {REGISTRY}", known.len());
    }

    println!("searching for Yeelight devices (3s)...");
    let devices = discovery::search(Duration::from_secs(3)).await?;
    if devices.is_empty() {
        println!("no devices found");
    }
    for d in &devices {
        println!(
            "- {} {:?} @ {}  power={:?} bright={:?}  ({} methods)",
            d.id,
            d.model,
            d.location,
            d.state.power,
            d.state.bright,
            d.support.len()
        );
    }

    if !devices.is_empty() {
        registry::save(REGISTRY, &devices)?;
        println!("saved {} device(s) to {REGISTRY}", devices.len());
    }
    Ok(())
}
