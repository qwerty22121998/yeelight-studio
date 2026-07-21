//! Set the saved device's ambient (background) light to red — a live write check.
//!
//! Run: `cargo run -p yeelight-core --example test_ambient_red`

use yeelight_core::{Client, Effect, registry};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = registry::load("yeelight-devices.toml")?
        .into_iter()
        .next()
        .ok_or("no saved device")?;
    println!("connecting to {} @ {}", device.id, device.location);

    let client = Client::connect(device).await?;
    client.bg_set_power(true, Effect::Smooth(500), None).await?;
    client.bg_set_rgb(0xFF0000, Effect::Smooth(500)).await?;

    let rgb = client.get_prop(&["bg_rgb"]).await?[0].clone();
    println!("bg_rgb -> {rgb}");
    assert_eq!(rgb, "16711680", "ambient not red (0xFF0000)");
    Ok(())
}
