//! LAN discovery via the Yeelight SSDP-like protocol (spec §3).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::time::Instant;

use crate::device::{Device, Model, State};
use crate::error::Result;

/// Multicast group used by Yeelight discovery.
pub const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
/// Static UDP port used for discovery (not standard SSDP 1900, spec §3).
pub const SSDP_PORT: u16 = 1982;

const SEARCH_MSG: &str =
    "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n";

/// Build the multicast UDP socket bound to the static discovery port.
///
/// `SO_REUSEADDR`/`SO_REUSEPORT` let several processes (and the passive [`Listener`])
/// share port 1982 at once.
fn bind_socket() -> Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    let bind: SocketAddr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, SSDP_PORT));
    sock.bind(&bind.into())?;
    sock.join_multicast_v4(&SSDP_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    sock.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(sock.into())?)
}

/// Actively search for devices, collecting unicast responses for `timeout`.
///
/// Sends a single `M-SEARCH` to the multicast group and gathers `HTTP/1.1 200 OK`
/// replies (spec §3.1), de-duplicated by device `id`.
pub async fn search(timeout: Duration) -> Result<Vec<Device>> {
    let socket = bind_socket()?;
    let target = SocketAddrV4::new(SSDP_ADDR, SSDP_PORT);
    socket.send_to(SEARCH_MSG.as_bytes(), target).await?;
    tracing::info!("sent M-SEARCH to {target}, collecting responses for {timeout:?}");

    let mut found: HashMap<String, Device> = HashMap::new();
    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; 2048];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, _src))) => {
                if let Ok(text) = std::str::from_utf8(&buf[..n])
                    && let Some(dev) = parse_headers(text)
                {
                    found.insert(dev.id.clone(), dev);
                }
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break, // overall timeout elapsed
        }
    }
    Ok(found.into_values().collect())
}

/// Passive listener for device advertisements (spec §3.2).
///
/// Devices multicast a `NOTIFY` right after joining the network and periodically after.
/// Receivers must never respond to advertisements (spec §3).
pub struct Listener {
    socket: UdpSocket,
    buf: Vec<u8>,
}

impl Listener {
    /// Bind to the static discovery port and join the multicast group.
    pub async fn bind() -> Result<Self> {
        Ok(Self {
            socket: bind_socket()?,
            buf: vec![0u8; 2048],
        })
    }

    /// Wait for the next valid advertisement and return the announced device.
    pub async fn recv(&mut self) -> Result<Device> {
        loop {
            let (n, _src) = self.socket.recv_from(&mut self.buf).await?;
            if let Ok(text) = std::str::from_utf8(&self.buf[..n])
                && let Some(dev) = parse_headers(text)
            {
                return Ok(dev);
            }
        }
    }
}

/// Parse a search response (`HTTP/1.1 200 OK`) or advertisement (`NOTIFY`) into a [`Device`].
///
/// Returns `None` for malformed messages (silently dropped, spec §3). Header names are
/// case-insensitive; only the first `:` splits a header line so `Location` URLs survive.
fn parse_headers(text: &str) -> Option<Device> {
    let mut lines = text.lines();
    let start = lines.next()?.trim();
    if !(start.starts_with("HTTP/1.1") || start.starts_with("NOTIFY")) {
        return None;
    }

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let id = headers.get("id")?.clone();
    let location: SocketAddr = headers.get("location")?.strip_prefix("yeelight://")?.parse().ok()?;
    let model = headers
        .get("model")
        .map(|s| Model::from(s.as_str()))
        .unwrap_or(Model::Unknown(String::new()));
    let fw_ver = headers.get("fw_ver").cloned().unwrap_or_default();
    let support = headers
        .get("support")
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let state = parse_state(&headers);

    Some(Device {
        id,
        model,
        fw_ver,
        location,
        support,
        state,
    })
}

fn parse_state(h: &HashMap<String, String>) -> State {
    State {
        // Multi-light devices report `main_power`; single-light `power`.
        power: h.get("power").or_else(|| h.get("main_power")).map(|p| p == "on"),
        bright: h.get("bright").and_then(|s| s.parse().ok()),
        color_mode: h.get("color_mode").and_then(|s| s.parse().ok()),
        ct: h.get("ct").and_then(|s| s.parse().ok()),
        rgb: h.get("rgb").and_then(|s| s.parse().ok()),
        hue: h.get("hue").and_then(|s| s.parse().ok()),
        sat: h.get("sat").and_then(|s| s.parse().ok()),
        name: h.get("name").cloned(),
        bg_power: h.get("bg_power").map(|p| p == "on"),
        bg_bright: h.get("bg_bright").and_then(|s| s.parse().ok()),
        bg_color_mode: h.get("bg_lmode").and_then(|s| s.parse().ok()),
        bg_ct: h.get("bg_ct").and_then(|s| s.parse().ok()),
        bg_rgb: h.get("bg_rgb").and_then(|s| s.parse().ok()),
        bg_hue: h.get("bg_hue").and_then(|s| s.parse().ok()),
        bg_sat: h.get("bg_sat").and_then(|s| s.parse().ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH_RESPONSE: &str = "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nDate:\r\nExt:\r\nLocation: yeelight://192.168.1.239:55443\r\nServer: POSIX UPnP/1.0 YGLC/1\r\nid: 0x000000000015243f\r\nmodel: color\r\nfw_ver: 18\r\nsupport: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb\r\npower: on\r\nbright: 100\r\ncolor_mode: 2\r\nct: 4000\r\nrgb: 16711680\r\nhue: 100\r\nsat: 35\r\nname: my_bulb\r\n";

    const ADVERTISEMENT: &str = "NOTIFY * HTTP/1.1\r\nHost: 239.255.255.250:1982\r\nCache-Control: max-age=3600\r\nLocation: yeelight://192.168.1.239:55443\r\nNTS: ssdp:alive\r\nServer: POSIX, UPnP/1.0 YGLC/1\r\nid: 0x000000000015243f\r\nmodel: color\r\nfw_ver: 18\r\nsupport: get_prop set_power\r\npower: off\r\nbright: 50\r\n";

    #[test]
    fn parses_search_response() {
        let d = parse_headers(SEARCH_RESPONSE).expect("valid response");
        assert_eq!(d.id, "0x000000000015243f");
        assert_eq!(d.model, Model::Color);
        assert_eq!(d.location, "192.168.1.239:55443".parse().unwrap());
        assert!(d.supports("set_rgb"));
        assert_eq!(d.state.power, Some(true));
        assert_eq!(d.state.bright, Some(100));
    }

    #[test]
    fn parses_advertisement() {
        let d = parse_headers(ADVERTISEMENT).expect("valid advertisement");
        assert_eq!(d.id, "0x000000000015243f");
        assert_eq!(d.state.power, Some(false));
        assert_eq!(d.state.bright, Some(50));
    }

    #[test]
    fn rejects_non_yeelight_start_line() {
        assert!(parse_headers("GARBAGE\r\nid: 0x1\r\n").is_none());
    }
}
