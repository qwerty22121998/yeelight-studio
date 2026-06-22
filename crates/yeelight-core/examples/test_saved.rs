//! Connect to the first saved device and read its props — a live round-trip check.
//!
//! Run: `cargo run -p yeelight-core --example test_saved`

use yeelight_core::{Client, registry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = registry::load("yeelight-devices.toml")?
        .into_iter()
        .next()
        .ok_or("no saved device")?;
    println!("connecting to {} @ {}", device.id, device.location);

    let client = Client::connect(device).await?;
    let props = client.get_prop(&["power", "bright", "rgb", "ct"]).await?;
    println!("props (power, bright, rgb, ct): {props:?}");

    let before = client.get_prop(&["power"]).await?[0].clone();
    client.toggle().await?;
    let after = client.get_prop(&["power"]).await?[0].clone();
    println!("toggle: power {before} -> {after}");
    assert_ne!(before, after, "toggle did not change power state");
    Ok(())
}
