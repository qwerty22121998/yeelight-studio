# yeelight-studio

Discover and locally control [Yeelight](https://www.yeelight.com/) WiFi LEDs over your LAN, from Rust. No cloud, no account — just the device's documented [LAN control protocol](docs/yeelight-spec.md).

The workspace contains a single library crate, `yeelight-core`.

## Features

- **Discovery** — active `search()` and a passive `Listener` over the Yeelight SSDP-like multicast protocol.
- **Typed control** — a `Client` per device with one method per protocol command (power, RGB/HSV/CT, brightness, color flow, scenes, cron, adjust), each validating its arguments before they hit the wire. Background-light (`bg_*`) twins included.
- **Async notifications** — subscribe to the device's pushed state-change stream.
- **Music mode** — `MusicConnection` for unthrottled, fire-and-forget streaming (the device connects back to you).
- **Registry** — persist discovered devices to TOML so later runs can skip a fresh search.
- **Firewall helpers** — open the static UDP/TCP ports via `ufw` on Linux.

## Usage

Add the crate to your workspace (it is not yet published to crates.io):

```toml
[dependencies]
yeelight-core = { path = "crates/yeelight-core" }
tokio = { version = "1", features = ["full"] }
```

Discover a device and set it red:

```rust
use std::time::Duration;
use yeelight_core::{discovery, firewall, Client, Effect};

#[tokio::main]
async fn main() -> yeelight_core::Result<()> {
    firewall::ensure_udp_open(discovery::SSDP_PORT).await?;
    let devices = discovery::search(Duration::from_secs(3)).await?;
    if let Some(device) = devices.into_iter().next() {
        let client = Client::connect(device).await?;
        client.set_power(true, Effect::Smooth(500), None).await?;
        client.set_rgb(0xFF0000, Effect::Smooth(500)).await?;
    }
    Ok(())
}
```

> **Note:** "LAN Control" must be enabled for each bulb in the Yeelight app, otherwise the control port stays closed.

## Examples

Runnable examples live in `crates/yeelight-core/examples/` and require a real bulb on your network:

```bash
cargo run -p yeelight-core --example discover   # find devices and print them
cargo run -p yeelight-core --example control    # connect and drive one device
cargo run -p yeelight-core --example music      # stream commands over music mode
```

## Development

```bash
cargo test                  # unit tests — no device or network needed
cargo clippy --all-targets  # lints
cargo doc --open            # full API docs
```

## License

MIT
