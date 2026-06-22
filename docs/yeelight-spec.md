# Yeelight WiFi Light Inter-Operation Specification

> Source: `Yeelight_Inter-Operation_Spec.pdf` — Qingdao Yeelink Information Technology Co., Ltd, 2015. <www.yeelight.com>

## 1. Introduction

Yeelight smart LED products support remote control over WiFi. On first use the device must be provisioned with the router's SSID/password via a proprietary procedure (SmartConfig / QuickConnect). Once on the router, the device is reachable by any device on the same network and can be controlled by 3rd-party equipment that speaks the inter-operation control protocol.

This document covers the technical details of **discovering** and **controlling** the device locally.

## 2. Overview

By default Yeelight WiFi LEDs are controlled through the cloud: commands go to a cloud server, then forwarded to the device. If the cloud or WAN is down, control is lost — hence the **local control** mechanism described here.

Local control has two parts:

- **Local discovery** — a simplified SSDP-like protocol (section 3).
- **Control protocol** — JSON control commands over TCP (section 4).

## 3. Local Discovery

SSDP-style discovery with two message kinds:

- **Search** — a device looks for others.
- **Advertise** — a device announces its presence.

Yeelight LEDs support both. The LED listens on a multicast address for search requests; if the request targets Yeelight (`ST: wifi_bulb`), it uni-casts a response with basic info (IP/port of control service, power, brightness, supported methods). It also multicasts an advertisement right after joining the network and periodically (state refresh). **Receivers must not respond to advertisements.**

> Multicast uses port **1982** (not standard SSDP 1900) to avoid flooding both LEDs and power-sensitive 3rd-party devices (e.g. battery smart watches).

### 3.1 Search request and response

Send to multicast `239.255.255.250:1982` over UDP:

```
M-SEARCH * HTTP/1.1
HOST: 239.255.255.250:1982
MAN: "ssdp:discover"
ST: wifi_bulb
```

Rules:

1. Start line must be `M-SEARCH * HTTP/1.1` with no leading whitespace.
2. `HOST` is optional; if present, value must be `239.255.255.250:1982`.
3. `MAN` is required; value must be `"ssdp:discover"` (double quotes included).
4. `ST` is required; value must be `wifi_bulb`.
5. Header names are case-insensitive; the start line and all header **values** are case-sensitive. Each line terminated by `\r\n`.

Messages not following the rules are silently dropped. A valid request gets a response uni-cast to the sender's IP:port (e.g. request from `192.168.1.22:43210` → response to `192.168.1.22:43210`):

```
HTTP/1.1 200 OK
Cache-Control: max-age=3600
Date:
Ext:
Location: yeelight://192.168.1.239:55443
Server: POSIX UPnP/1.0 YGLC/1
id: 0x000000000015243f
model: color
fw_ver: 18
support: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb
power: on
bright: 100
color_mode: 2
ct: 4000
rgb: 16711680
hue: 100
sat: 35
name: my_bulb
```

| Header | Meaning |
|---|---|
| start line | always `HTTP/1.1 200 OK` |
| `Cache-Control` | status refresh interval; next advertisement comes after this many seconds |
| `Location` | service access point. Scheme always `yeelight`, host = LED IP, port = control-service TCP port |
| `Date` / `Ext` / `Server` | no important info; present only for SSDP compatibility |
| `id` | unique LED device ID; use to identify a device |
| `model` | product model: `mono`, `color`, `stripe`, `ceiling`, `bslamp` (more may be added). `mono` = brightness only; `color` = color + color temperature; `stripe` = LED stripe; `ceiling` = ceiling light |
| `fw_ver` | firmware version |
| `support` | whitespace-separated list of supported control methods. Any method not in this list is rejected by the LED |
| `power` | `on` / `off` (software-managed, not un-powered) |
| `bright` | brightness percentage, `1 ~ 100` |
| `color_mode` | `1` color mode / `2` color-temperature mode / `3` HSV mode |
| `ct` | color temperature; valid only if `color_mode` = 2 |
| `rgb` | RGB value; valid only if `color_mode` = 1 |
| `hue` | `0 ~ 359`; valid only if `color_mode` = 3 |
| `sat` | `0 ~ 100`; valid only if `color_mode` = 3 |
| `name` | device name, set via `set_name`, max 64 bytes. Non-ASCII names should be BASE64'd first |

> **NOTE:** HUE and SAT must be used together. CT, RGB and HSV modes are mutually exclusive.

Recommended handling after receiving the response:

1. Parse and validate.
2. Identify the device by `id` against local storage.
3. Display status if needed.
4. Use `Location` to open a TCP connection to the LED.
5. Send control messages / monitor state and reflect changes to the user.

### 3.2 Advertisement

Right after joining the network the LED multicasts an advertisement so 3rd-party devices know it is online (avoids power-wasteful polling). It then refreshes state at a fixed interval:

```
NOTIFY * HTTP/1.1
Host: 239.255.255.250:1982
Cache-Control: max-age=3600
Location: yeelight://192.168.1.239:55443
NTS: ssdp:alive
Server: POSIX, UPnP/1.0 YGLC/1
id: 0x000000000015243f
model: color
fw_ver: 18
support: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb
power: on
bright: 100
color_mode: 2
ct: 4000
rgb: 16711680
hue: 100
sat: 35
name: my_bulb
```

- Start line always `NOTIFY * HTTP/1.1`.
- `NTS` value always `ssdp:alive`.
- `Cache-Control`, `Location` as in 3.1. All other Yeelight-specific headers are identical to the search response (section 3.1).

## 4. Control Protocol

After discovery, a control plane is established between 3rd-party device and LED over TCP, carrying JSON messages. Three message types: **COMMAND**, **RESULT**, **NOTIFICATION**. Each message terminated by `\r\n`. You can `telnet <IP> 55443` and send commands by hand for debugging.

> **NOTE — Limits:** Up to **4 simultaneous TCP connections**; further connects rejected. Per connection: **60 commands/minute**. Total LAN quota across all connections: **144 commands/minute** (4 × 60 × 60%).

### 4.1 COMMAND message

Generated by the 3rd-party device, sent to the LED:

```
{ "id": <int>, "method": "<string>", "params": [<array>] }\r\n
```

| Pair | Presence | Key | Value |
|---|---|---|---|
| id | mandatory | `"id"` | int |
| method | mandatory | `"method"` | string |
| params | mandatory | `"params"` | array |

- `id` — integer chosen by sender; echoed back in the RESULT message to correlate request/response.
- `method` — must be one of the methods listed in the `support` header. Others are rejected.
- `params` — method-specific array.

Example:

```json
{ "id": 1, "method": "set_power", "params": ["on", "smooth", 500] }
```

#### Method summary (Table 4-1)

| Method | Params count | Param 1 | Param 2 | Param 3 | Param 4 |
|---|---|---|---|---|---|
| `get_prop` | 1 ~ N | * | * | * | * |
| `set_ct_abx` | 3 | int(ct_value) | string(effect) | int(duration) | |
| `set_rgb` | 3 | int(rgb_value) | string(effect) | int(duration) | |
| `set_hsv` | 4 | int(hue) | int(sat) | string(effect) | int(duration) |
| `set_bright` | 3 | int(brightness) | string(effect) | int(duration) | |
| `set_power` | 3 | string(power) | string(effect) | int(duration) | int(mode) |
| `toggle` | 0 | | | | |
| `set_default` | 0 | | | | |
| `start_cf` | 3 | int(count) | int(action) | string(flow_expression) | |
| `stop_cf` | 0 | | | | |
| `set_scene` | 3 ~ 4 | string(class) | int(val1) | int(val2) | int(val3) |
| `cron_add` | 2 | int(type) | int(value) | | |
| `cron_get` | 1 | int(type) | | | |
| `cron_del` | 1 | int(type) | | | |
| `set_adjust` | 2 | string(action) | string(prop) | | |
| `set_music` | 1 ~ 3 | int(action) | string(host) | int(port) | |
| `set_name` | 1 | string(name) | | | |
| `bg_set_rgb` | 3 | int(rgb_value) | string(effect) | int(duration) | |
| `bg_set_hsv` | 4 | int(hue) | int(sat) | string(effect) | int(duration) |
| `bg_set_ct_abx` | 3 | int(ct_value) | string(effect) | int(duration) | |
| `bg_start_cf` | 3 | int(count) | int(action) | string(flow_expression) | |
| `bg_stop_cf` | 0 | | | | |
| `bg_set_scene` | 3 ~ 4 | string(class) | int(val1) | int(val2) | int(val3) |
| `bg_set_default` | 0 | | | | |
| `bg_set_power` | 3 | string(power) | string(effect) | int(duration) | int(mode) |
| `bg_set_bright` | 3 | int(brightness) | string(effect) | int(duration) | |
| `bg_set_adjust` | 2 | string(action) | string(prop) | | |
| `bg_toggle` | 0 | | | | |
| `dev_toggle` | 0 | | | | |
| `adjust_bright` | 2 | int(percentage) | int(duration) | | |
| `adjust_ct` | 2 | int(percentage) | int(duration) | | |
| `adjust_color` | 2 | int(percentage) | int(duration) | | |
| `bg_adjust_bright` | 2 | int(percentage) | int(duration) | | |
| `bg_adjust_ct` | 2 | int(percentage) | int(duration) | | |
| `bg_adjust_color` | 2 | int(percentage) | int(duration) | | |

#### Method details

**`get_prop`** — retrieve current properties. Params: 1 to N property names. Response contains corresponding values; unrecognized properties return `""`.

- Request: `{"id":1,"method":"get_prop","params":["power", "not_exist", "bright"]}`
- Response: `{"id":1, "result":["on", "", "100"]}`
- Supported properties: see Table 4-2 (section 4.3).

**`set_ct_abx`** — change color temperature. Params: 3.

- `ct_value` — target color temperature, int, range `1700 ~ 6500` (K).
- `effect` — `"sudden"` (jump directly, `duration` ignored) or `"smooth"` (gradual over `duration`).
- `duration` — total gradual-change time in ms; minimum 30.
- Request: `{"id":1,"method":"set_ct_abx","params":[3500, "smooth", 500]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`set_rgb`** — change color. Params: 3.

- `rgb_value` — target color, int, range `0 ~ 16777215` (`0xFFFFFF`).
- `effect` / `duration` — as `set_ct_abx`.
- RGB format: 24 bits, `RGB = (R*65536) + (G*256) + B`.
- Request: `{"id":1,"method":"set_rgb","params":[255, "smooth", 500]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`set_hsv`** — change color. Params: 4.

- `hue` — int, range `0 ~ 359`.
- `sat` — int, range `0 ~ 100`.
- `effect` / `duration` — as `set_ct_abx`.
- Request: `{"id":1,"method":"set_hsv","params":[255, 45, "smooth", 500]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`set_bright`** — change brightness. Params: 3.

- `brightness` — int percentage `1 ~ 100` (1 = min, 100 = max).
- `effect` / `duration` — as `set_ct_abx`.
- Request: `{"id":1,"method":"set_bright","params":[50, "smooth", 500]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`set_power`** — turn on/off (software-managed). Params: 3.

- `power` — `"on"` or `"off"`.
- `effect` / `duration` — as `set_ct_abx`.
- `mode` (optional):
  - `0` normal turn on (default)
  - `1` turn on and switch to CT mode
  - `2` turn on and switch to RGB mode
  - `3` turn on and switch to HSV mode
  - `4` turn on and switch to color-flow mode
  - `5` turn on and switch to night-light mode (ceiling light only)
- Request: `{"id":1,"method":"set_power","params":["on", "smooth", 500]}`
- Response: `{"id":1, "result":["ok"]}`

**`toggle`** — flip on/off. Params: 0. Defined so a user can flip state without knowing the current state.

- Request: `{"id":1,"method":"toggle","params":[]}`
- Response: `{"id":1, "result":["ok"]}`

**`set_default`** — save current state to persistent memory; restored after a hard power reset. Params: 0.

- Request: `{"id":1,"method":"set_default","params":[]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`start_cf`** — start a color flow (series of visible state changes: brightness, color, or CT). Most powerful command; recommended scenes (sunrise/sunset) use it. Params: 3.

- `count` — number of visible state changes before stopping; `0` = infinite loop.
- `action` — action after the flow stops:
  - `0` recover to the state before the flow started
  - `1` stay at the state when the flow stopped
  - `2` turn off the LED after the flow stopped
- `flow_expression` — the state-changing series. A series of **flow tuples** `[duration, mode, value, brightness]`:
  - `duration` — gradual change / sleep time in ms, minimum 50.
  - `mode` — `1` color, `2` color temperature, `7` sleep.
  - `value` — RGB value when mode=1, CT value when mode=2, ignored when mode=7.
  - `brightness` — `-1` or `1 ~ 100`; ignored when mode=7. `-1` keeps current brightness (only color/CT changes).
- Request: `{"id":1,"method":"start_cf","params":[ 4, 2, "1000, 2, 2700, 100, 500, 1, 255, 10, 5000, 7, 0,0, 500, 2, 5000, 1"]}`
- Response: `{"id":1, "result":["ok"]}`
- The example: gradually go to 2700K & max brightness over 1000ms, then red & 10% over 500ms, then sleep 5s, then 5000K & min brightness over 500ms; after 4 changes stop and power off.
- **NOTE:** only accepted when the LED is `on`.

Pseudo-code:

```
+start_cf:
  cnt = 0
  while true:
    if flow_cnt != 0 and cnt >= flow_cnt:
      take_stop_action(flow_action)
      break
    tuple = get_next_flow_tuple()   # flow tuple put in a circular list
    apply_effect(tuple)             # change RGB/CT gradually or sleep
```

**`stop_cf`** — stop a running color flow. Params: 0.

- Request: `{"id":1,"method":"stop_cf","params":[]}`
- Response: `{"id":1, "result":["ok"]}`

**`set_scene`** — set the LED directly to a state. If off, turns on first then applies. Params: 3 ~ 4.

- `class`:
  - `"color"` — set color + brightness.
  - `"hsv"` — set hue/sat + brightness.
  - `"ct"` — set CT + brightness.
  - `"cf"` — start a color flow.
  - `"auto_delay_off"` — turn on to a brightness and start a sleep timer (minutes).
- `val1` / `val2` / `val3` — class-specific.
- Requests:
  - `{"id":1,"method":"set_scene", "params": ["color", 65280, 70]}`
  - `{"id":1,"method":"set_scene", "params": ["hsv", 300, 70, 100]}`
  - `{"id":1,"method":"set_scene", "params":["ct", 5400, 100]}`
  - `{"id":1,"method":"set_scene","params":["cf",0,0,"500,1,255,100,1000,1,16776960,70"]}`
  - `{"id":1,"method":"set_scene","params":["auto_delay_off", 50, 5]}`
- Response: `{"id":1, "result":["ok"]}`
- Accepted in both `on` and `off` state. (1: color 65280 @ 70%. 2: HSV 300/70 @ max. 3: 5400K @ 100%. 4: infinite color flow on two tuples. 5: 50% brightness, off after 5 min.)

**`cron_add`** — start a timer job. Params: 2.

- `type` — currently only `0` (power off).
- `value` — timer length in minutes.
- Request: `{"id":1,"method":"cron_add","params":[0, 15]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** only accepted when the LED is `on`.

**`cron_get`** — retrieve current cron job. Params: 1.

- `type` — cron type (currently only `0`).
- Request: `{"id":1,"method":"cron_get","params":[0]}`
- Response: `{"id":1, "result":[{"type": 0, "delay": 15, "mix": 0}]}`

**`cron_del`** — stop the specified cron job. Params: 1.

- `type` — cron type (currently only `0`).
- Request: `{"id":1,"method":"cron_del","params":[0]}`
- Response: `{"id":1, "result":["ok"]}`

**`set_adjust`** — change brightness/CT/color without knowing the current value (mainly for controllers). Params: 2.

- `action` — `"increase"`, `"decrease"`, `"circle"` (increase, then wrap to min after max).
- `prop` — `"bright"`, `"ct"`, `"color"`. When `prop` is `"color"`, `action` can only be `"circle"`; otherwise the request is invalid.
- Request: `{"id":1,"method":"set_adjust","params":["increase", "ct"]}`
- Response: `{"id":1, "result":["ok"]}`

**`set_music`** — start/stop music mode. Under music mode no properties are reported and **no message quota is checked**. Params: 1 ~ 3.

- `action` — `0` off, `1` on.
- `host` — IP of the music server.
- `port` — TCP port the music app listens on.
- Requests:
  - `{"id":1,"method":"set_music","params":[1, "192.168.0.2", 54321]}`
  - `{"id":1,"method":"set_music","params":[0]}`
- Response: `{"id":1, "result":["ok"]}`
- **Flow:** the controller starts a TCP server, then calls `set_music` with its IP/port. The LED connects back; once connected, the controller can send unlimited commands over that channel to simulate any music effect. Stop by sending an explicit stop or by closing the socket.

**`set_name`** — name the device; stored on the device and reported in discovery. Params: 1.

- `name` — the device name.
- Request: `{"id":1,"method":"set_name","params":["my_bulb"]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** the official app stores the name in the cloud; this stores it on device memory, so the two names may differ.

**`bg_set_*` / `bg_toggle`** — control the background light; semantics mirror the matching `set_*` command. Only supported on lights equipped with a background light.

**`dev_toggle`** — toggle main light and background light at the same time. (`toggle` = main only, `bg_toggle` = background only, `dev_toggle` = both.)

**`adjust_bright`** — adjust brightness by a percentage over a duration. Params: 2.

- `percentage` — `-100 ~ 100`.
- `duration` — as `set_ct_abx`.
- Request: `{"id":1,"method":"adjust_bright","params":[-20, 500]}` (decrease brightness 20% over 500ms)
- Response: `{"id":1, "result":["ok"]}`

**`adjust_ct`** — adjust color temperature by a percentage over a duration. Params: 2.

- `percentage` — `-100 ~ 100`.
- `duration` — as `set_ct_abx`.
- Request: `{"id":1,"method":"adjust_ct","params":[20, 500]}` (increase CT 20% over 500ms)
- Response: `{"id":1, "result":["ok"]}`

**`adjust_color`** — adjust color over a duration. Params: 2.

- `percentage` — `-100 ~ 100`.
- `duration` — as `set_ct_abx`.
- Request: `{"id":1,"method":"adjust_color","params":[20, 500]}`
- Response: `{"id":1, "result":["ok"]}`
- **NOTE:** the percentage is ignored; the color is internally defined and cannot be specified.

**`bg_adjust_*`** — adjust the background light by percentage over duration. Refer to `adjust_bright` / `adjust_ct` / `adjust_color`.

### 4.2 RESULT message

Generated by the LED on receiving a COMMAND. Every command expects a result.

```
{ "id": <int>, "result"/"error": <array or object> }\r\n
```

| Pair | Presence | Key | Value |
|---|---|---|---|
| id | mandatory | `"id"` | int |
| result | mandatory | `"result"` / `"error"` | array(value) or object(value) |

- `id` — mirrors the COMMAND `id`; used by sender to correlate, meaningless to the LED.
- On success, `result` is returned — an array of `"ok"` or the requested property values (`get_prop`).
- On failure, `error` is returned — an object with a detailed error description.

Examples:

```json
{"id":1, "result":["ok"]}
{"id":2, "error":{"code":-1, "message":"unsupported method"}}
{"id":3, "result":["on","100"]}
```

(The third is the response to `{"id":3,"method":"get_prop","params":["power","bright"]}`.)

### 4.3 NOTIFICATION message

Whenever the LED's state changes, it sends a notification to all connected 3rd-party devices so they get the latest state without polling.

```
{ "method": "<string>", "params": {<object>} }\r\n
```

| Pair | Presence | Key | Value |
|---|---|---|---|
| method | mandatory | `"method"` | string |
| params | mandatory | `"params"` | object |

The `params` object holds property name → value pairs (**all values are String type**):

| Pair | Presence | Key | Value |
|---|---|---|---|
| prop_val_pair | mandatory | property name | string (property value) |

- `method` — currently only `"props"`. Any other value = invalid notification.

Example (LED switched on to 10% brightness):

```json
{"method":"props","params":{"power":"on", "bright":"10"}}
```

#### Supported properties (Table 4-2)

| Property | Possible value |
|---|---|
| `power` | `on` / `off` |
| `bright` | brightness percentage, range `1 ~ 100` |
| `ct` | color temperature, range `1700 ~ 6500` (K) |
| `rgb` | color, range `1 ~ 16777215` |
| `hue` | hue, range `0 ~ 359` |
| `sat` | saturation, range `0 ~ 100` |
| `color_mode` | `1` rgb mode / `2` color-temperature mode / `3` hsv mode |
| `flowing` | `0` no flow running / `1` color flow running |
| `delayoff` | remaining sleep-timer time, range `1 ~ 60` (minutes) |
| `flow_params` | current flow parameters (meaningful only when `flowing` is 1) |
| `music_on` | `1` music mode on / `0` off |
| `name` | device name set by `set_name` |
| `bg_power` | background light power status |
| `bg_flowing` | background light is flowing |
| `bg_flow_params` | current flow parameters of background light |
| `bg_ct` | color temperature of background light |
| `bg_lmode` | `1` rgb / `2` color-temperature / `3` hsv mode (background light) |
| `bg_bright` | brightness percentage of background light |
| `bg_rgb` | color of background light |
| `bg_hue` | hue of background light |
| `bg_sat` | saturation of background light |
| `nl_br` | brightness of night-mode light |
| `active_mode` | `0` daylight mode / `1` moonlight mode (ceiling light only) |

## 5. Issues and Future Consideration

This spec will be updated if the Yeelight local control protocol changes.

## 6. Reference

- SSDP: <https://tools.ietf.org/html/draft-cai-ssdp-v1-03>
- JSON: <http://www.json.org/>
