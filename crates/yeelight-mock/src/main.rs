//! Fake Yeelight bulbs for local development without real hardware.
//!
//! Each bulb answers discovery M-SEARCHes (so `yeelight_core::discovery::search`
//! and the GUI's Scan find it) and serves the JSON-over-TCP control protocol on
//! loopback, keeping a tiny in-memory state that commands mutate and echo back as
//! `props` notifications. One shared responder replies once per bulb so every mock
//! shows up in a single scan.
//!
//! Run: `cargo run -p yeelight-mock -- [COUNT]` (default 1), or `make mock N=3`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use yeelight_core::discovery::{SSDP_ADDR, SSDP_PORT};

/// Methods every mock advertises. A real bulb's set varies by model; a dev mock
/// claims everything so all GUI controls are live.
const SUPPORT: &[&str] = &[
    "get_prop", "set_power", "toggle", "set_bright", "start_cf", "stop_cf", "set_scene",
    "set_ct_abx", "set_rgb", "set_hsv", "set_default", "set_name", "set_adjust", "adjust_bright",
    "adjust_ct", "adjust_color", "set_music", "cron_add", "cron_get", "cron_del", "bg_set_power",
    "bg_toggle", "bg_set_bright", "bg_set_rgb", "bg_set_ct_abx", "bg_set_hsv", "bg_set_scene",
    "bg_set_default", "dev_toggle",
];

/// Immutable discovery facts for one bulb.
struct Meta {
    id: String,
    location: SocketAddr,
    model: String,
    fw_ver: String,
    support: String,
}

/// A running mock: its discovery meta plus the live state shared with its control task.
struct Bulb {
    meta: Meta,
    state: Arc<Mutex<HashMap<String, String>>>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt().init();

    let count: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1);

    let mut bulbs = Vec::new();
    for i in 0..count {
        let b = spawn_control(i).await?;
        println!("mock bulb {i}: id={} control=yeelight://{}", b.meta.id, b.meta.location);
        bulbs.push(b);
    }

    // Discovery is best-effort: where multicast is unavailable (sandboxes, some
    // containers) the bulbs are still reachable by a direct Client::connect.
    match bind_discovery_socket() {
        Ok(sock) => {
            println!("discovery responder on udp/{SSDP_PORT}");
            tokio::spawn(run_discovery(sock, bulbs));
        }
        Err(e) => eprintln!("discovery disabled (multicast unavailable): {e}"),
    }

    println!("serving {count} mock bulb(s); Ctrl-C to stop");
    std::future::pending::<()>().await;
    Ok(())
}

/// Bind a loopback TCP control server for one bulb and return its discovery handle.
async fn spawn_control(index: u32) -> std::io::Result<Bulb> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let location = listener.local_addr()?;
    // Unique, stable per port so repeated scans dedupe to the same device.
    let id = format!("0x{:016x}", location.port());

    let state: HashMap<String, String> = [
        ("power", "off"),
        ("bright", "100"),
        ("color_mode", "2"),
        ("ct", "4000"),
        ("rgb", "16777215"),
        ("hue", "0"),
        ("sat", "0"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .chain([("name".to_string(), format!("mock-{index}"))])
    .collect();
    let state = Arc::new(Mutex::new(state));

    tokio::spawn(accept_loop(listener, Arc::clone(&state)));

    Ok(Bulb {
        meta: Meta {
            id,
            location,
            model: "color".to_string(),
            fw_ver: "1".to_string(),
            support: SUPPORT.join(" "),
        },
        state,
    })
}

/// Accept control connections forever, one task per connection.
async fn accept_loop(listener: TcpListener, state: Arc<Mutex<HashMap<String, String>>>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "control connection");
                tokio::spawn(handle_conn(stream, Arc::clone(&state)));
            }
            Err(e) => {
                tracing::error!("accept: {e}");
                break;
            }
        }
    }
}

/// Read `\r\n`-delimited command lines and answer each.
async fn handle_conn(stream: TcpStream, state: Arc<Mutex<HashMap<String, String>>>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        if respond(&mut write, &state, &line).await.is_err() {
            break;
        }
    }
}

/// Apply one command, write its result, and push a `props` notification for any change.
async fn respond(
    w: &mut OwnedWriteHalf,
    state: &Mutex<HashMap<String, String>>,
    line: &str,
) -> std::io::Result<()> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("bad command {line:?}: {e}");
            return Ok(());
        }
    };
    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
    let method = v.get("method").and_then(Value::as_str).unwrap_or_default();
    let empty: Vec<Value> = Vec::new();
    let params = v.get("params").and_then(Value::as_array).unwrap_or(&empty);

    let (result, notif) = {
        let mut s = state.lock().await;
        let (result, changed) = apply(&mut s, method, params);
        let notif = (!changed.is_empty()).then(|| {
            let props: serde_json::Map<String, Value> = changed
                .iter()
                .map(|k| (k.to_string(), Value::String(s.get(*k).cloned().unwrap_or_default())))
                .collect();
            json!({ "method": "props", "params": props })
        });
        (result, notif)
    };

    write_line(w, &json!({ "id": id, "result": result })).await?;
    if let Some(n) = notif {
        write_line(w, &n).await?;
    }
    Ok(())
}

/// Mutate state for `method` and return `(result, changed_prop_names)`.
/// Unknown methods (and `bg_*`, which this mock does not model) just return `["ok"]`.
fn apply(
    state: &mut HashMap<String, String>,
    method: &str,
    params: &[Value],
) -> (Value, Vec<&'static str>) {
    let ok = json!(["ok"]);
    match method {
        "get_prop" => {
            let vals: Vec<Value> = params
                .iter()
                .filter_map(Value::as_str)
                .map(|k| Value::String(state.get(k).cloned().unwrap_or_default()))
                .collect();
            (Value::Array(vals), vec![])
        }
        "set_power" => {
            set(state, "power", params, 0);
            (ok, vec!["power"])
        }
        "toggle" | "dev_toggle" => {
            let on = state.get("power").map(|p| p == "on").unwrap_or(false);
            state.insert("power".to_string(), if on { "off" } else { "on" }.to_string());
            (ok, vec!["power"])
        }
        "set_bright" => {
            set(state, "bright", params, 0);
            (ok, vec!["bright"])
        }
        "set_rgb" => {
            set(state, "rgb", params, 0);
            state.insert("color_mode".to_string(), "1".to_string());
            (ok, vec!["rgb", "color_mode"])
        }
        "set_ct_abx" => {
            set(state, "ct", params, 0);
            state.insert("color_mode".to_string(), "2".to_string());
            (ok, vec!["ct", "color_mode"])
        }
        "set_hsv" => {
            set(state, "hue", params, 0);
            set(state, "sat", params, 1);
            state.insert("color_mode".to_string(), "3".to_string());
            (ok, vec!["hue", "sat", "color_mode"])
        }
        "set_name" => {
            set(state, "name", params, 0);
            (ok, vec!["name"])
        }
        _ => (ok, vec![]),
    }
}

/// Store `params[idx]` (as its string form, per the wire protocol) under `key`.
fn set(state: &mut HashMap<String, String>, key: &str, params: &[Value], idx: usize) {
    if let Some(v) = params.get(idx) {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        state.insert(key.to_string(), s);
    }
}

/// Serialize `v` as one `\r\n`-terminated line.
async fn write_line(w: &mut OwnedWriteHalf, v: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(v).unwrap_or_default();
    line.push_str("\r\n");
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

/// Answer every M-SEARCH with one `HTTP/1.1 200 OK` per bulb.
async fn run_discovery(sock: UdpSocket, bulbs: Vec<Bulb>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                tracing::error!("discovery recv: {e}");
                return;
            }
        };
        let Ok(text) = std::str::from_utf8(&buf[..n]) else { continue };
        if !text.starts_with("M-SEARCH") {
            continue; // ignore our own loopback NOTIFY/HTTP, etc.
        }
        for b in &bulbs {
            let resp = {
                let s = b.state.lock().await;
                search_response(&b.meta, &s)
            };
            let _ = sock.send_to(resp.as_bytes(), src).await;
        }
        tracing::debug!(%src, bulbs = bulbs.len(), "answered M-SEARCH");
    }
}

/// Build the discovery response a real bulb would unicast back (spec §3.1).
fn search_response(m: &Meta, state: &HashMap<String, String>) -> String {
    use std::fmt::Write;
    let g = |k: &str| state.get(k).cloned().unwrap_or_default();
    let mut r = String::new();
    let _ = write!(
        r,
        "HTTP/1.1 200 OK\r\n\
         Cache-Control: max-age=3600\r\n\
         Location: yeelight://{loc}\r\n\
         id: {id}\r\n\
         model: {model}\r\n\
         fw_ver: {fw}\r\n\
         support: {sup}\r\n\
         power: {power}\r\n\
         bright: {bright}\r\n\
         color_mode: {cm}\r\n\
         ct: {ct}\r\n\
         rgb: {rgb}\r\n\
         hue: {hue}\r\n\
         sat: {sat}\r\n\
         name: {name}\r\n",
        loc = m.location,
        id = m.id,
        model = m.model,
        fw = m.fw_ver,
        sup = m.support,
        power = g("power"),
        bright = g("bright"),
        cm = g("color_mode"),
        ct = g("ct"),
        rgb = g("rgb"),
        hue = g("hue"),
        sat = g("sat"),
        name = g("name"),
    );
    r
}

/// Bind the shared multicast discovery socket, mirroring `yeelight_core`'s own bind
/// (`SO_REUSEADDR`/`SO_REUSEPORT` so the mock coexists with real listeners on 1982).
fn bind_discovery_socket() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    let bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, SSDP_PORT));
    sock.bind(&bind.into())?;
    sock.join_multicast_v4(&SSDP_ADDR, &Ipv4Addr::UNSPECIFIED)?;
    sock.set_nonblocking(true)?;
    UdpSocket::from_std(sock.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_mutates_and_reports_changes() {
        let mut s = HashMap::new();

        let (r, changed) = apply(&mut s, "set_power", &[json!("on")]);
        assert_eq!(r, json!(["ok"]));
        assert_eq!(changed, vec!["power"]);
        assert_eq!(s.get("power").unwrap(), "on");

        // get_prop echoes stored values; missing props come back as "".
        let (props, _) = apply(&mut s, "get_prop", &[json!("power"), json!("missing")]);
        assert_eq!(props, json!(["on", ""]));

        // numeric params are stored as their string form, and set_rgb flips color_mode.
        let (_, c) = apply(&mut s, "set_rgb", &[json!(16711680)]);
        assert_eq!(c, vec!["rgb", "color_mode"]);
        assert_eq!(s.get("rgb").unwrap(), "16711680");
        assert_eq!(s.get("color_mode").unwrap(), "1");

        // toggle flips power without params.
        let (_, c) = apply(&mut s, "toggle", &[]);
        assert_eq!(c, vec!["power"]);
        assert_eq!(s.get("power").unwrap(), "off");

        // unknown method is a no-op ok.
        let (r, c) = apply(&mut s, "bg_set_power", &[json!("on")]);
        assert_eq!(r, json!(["ok"]));
        assert!(c.is_empty());
    }
}
