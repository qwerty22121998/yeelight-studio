//! Application state, the update loop, and the async command plumbing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use iced::{Color, Element, Subscription, Task};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use serde_json::{Value, json};
use yeelight_core::{
    Client, CronType, DEFAULT_MUSIC_PORT, Direction, Effect, FlowAction, FlowExpr, FlowTuple,
    MusicConnection, State, discovery, firewall,
};

use crate::message::{Btn, CmdKind, FlowField, Message, Op, OpKey, SettingsTab, ThemePref};
use crate::view;

/// Transition used for every GUI-issued command.
pub(crate) const EFFECT: Effect = Effect::Smooth(500);

/// After a local interaction, ignore device notifications for this long before
/// reconciling the controls — so a drag (or the device echoing our own command)
/// can't yank the sliders out from under the user.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(1000);

/// Cap on in-memory command-log entries kept for the logging screen (the on-disk
/// file is unbounded). Oldest entries are dropped past this.
const LOG_CAP: usize = 500;

/// One captured wire line for the command-log screen.
pub(crate) struct LogEntry {
    /// When it was captured (used for the on-screen timestamp).
    pub(crate) time: SystemTime,
    /// Device id the line belongs to.
    pub(crate) device: String,
    /// Sent or received.
    pub(crate) direction: Direction,
    /// The raw JSON line.
    pub(crate) line: String,
}

/// Per-device control UI state (color-picker overlays + slider drafts).
pub(crate) struct PickerState {
    pub(crate) main_open: bool,
    pub(crate) main_draft: Color,
    pub(crate) bg_open: bool,
    pub(crate) bg_draft: Color,
    pub(crate) main_bright: u8,
    pub(crate) bg_bright: u8,
    pub(crate) main_ct: u16,
    pub(crate) bg_ct: u16,
    /// Light-tab segment override: `Some(true)` = temperature, `Some(false)` =
    /// color, `None` = follow the device's reported mode (main) or default (bg).
    pub(crate) main_seg: Option<bool>,
    pub(crate) bg_seg: Option<bool>,
}

impl Default for PickerState {
    fn default() -> Self {
        Self {
            main_open: false,
            main_draft: Color::WHITE,
            bg_open: false,
            bg_draft: Color::WHITE,
            main_bright: 100,
            bg_bright: 100,
            main_ct: 4000,
            bg_ct: 4000,
            main_seg: None,
            bg_seg: None,
        }
    }
}

/// One editable row of the custom flow editor (raw strings until Start).
#[derive(Clone)]
pub(crate) struct FlowRow {
    /// Duration in milliseconds (raw string input).
    pub(crate) duration: String,
    /// Flow mode: 1 = color, 2 = CT, 7 = sleep.
    pub(crate) mode: u8,
    /// RGB or CT value (raw string input).
    pub(crate) value: String,
    /// Brightness percentage (raw string input).
    pub(crate) bright: String,
}

impl Default for FlowRow {
    fn default() -> Self {
        Self { duration: "1000".into(), mode: 1, value: "16711680".into(), bright: "100".into() }
    }
}

impl FlowRow {
    /// Update one field from a raw input string.
    pub(crate) fn set(&mut self, field: FlowField, v: String) {
        match field {
            FlowField::Duration => self.duration = v,
            FlowField::Mode => self.mode = v.parse().unwrap_or(self.mode),
            FlowField::Value => self.value = v,
            FlowField::Bright => self.bright = v,
        }
    }
}

/// Per-device timer state: remaining whole seconds while a sleep timer runs.
#[derive(Default, Clone, Copy)]
pub(crate) struct TimerState {
    /// Remaining seconds, or `None` if no timer is active.
    pub(crate) remaining: Option<u32>,
}

/// A running ambient session: the live-config sender (cloned into the driver) plus
/// the resolved sink.
pub(crate) struct AmbientRun {
    /// Pushes live region/mode/target edits to the running driver.
    pub(crate) cfg_tx: tokio::sync::watch::Sender<crate::ambient::AmbientConfig>,
    /// The sink the driver sends through.
    pub(crate) sink: crate::ambient::AmbientSink,
    /// Fixed capture monitor (recipe key — changing it restarts capture).
    pub(crate) monitor_id: Option<u32>,
}

/// Bottom-bar status line.
#[derive(Default)]
pub(crate) enum Status {
    /// Nothing to report yet.
    #[default]
    Idle,
    /// Last successful action.
    Ok(String),
    /// Last error.
    Err(String),
}

/// Root application state.
pub(crate) struct App {
    /// Device screen vs settings screen.
    pub(crate) screen: crate::message::Screen,
    pub(crate) devices: Vec<yeelight_core::Device>,
    pub(crate) selected: Option<usize>,
    pub(crate) clients: HashMap<String, Arc<Client>>,
    pub(crate) pickers: HashMap<String, PickerState>,
    /// Ignore each device's `support` set: show every control and send regardless.
    pub(crate) force_all: bool,
    /// Which settings sub-tab is open.
    pub(crate) settings_tab: SettingsTab,
    /// Theme preference (the pick-list selection).
    pub(crate) theme_pref: ThemePref,
    /// Resolved theme handed to iced (cached so System isn't re-detected per frame).
    pub(crate) theme: iced::Theme,
    pub(crate) timeout_secs: u32,
    pub(crate) status: Status,
    /// A scan is in flight.
    pub(crate) scanning: bool,
    /// Scan progress `0.0..=1.0` (driven by [`Message::Tick`] over the timeout).
    pub(crate) scan_progress: f32,
    /// Buttons whose command is awaiting a reply (so only they disable).
    pub(crate) inflight: HashSet<OpKey>,
    /// Whether we've already offered to open the firewall this session (after an
    /// empty scan). Guards the offer so it pops at most once, not on every rescan.
    pub(crate) asked_open: bool,
    /// Active control tab per `(device id, background?)` — each light tabs apart.
    pub(crate) tabs: HashMap<(String, bool), crate::message::DetailTab>,
    /// In-progress rename buffer (device id, new name), if editing.
    pub(crate) rename: Option<(String, String)>,
    /// Custom flow-editor draft rows per `(device id, background?)`.
    pub(crate) flow_rows: HashMap<(String, bool), Vec<FlowRow>>,
    /// Custom flow repeat-count input per `(device id, background?)` (raw string).
    pub(crate) flow_count: HashMap<(String, bool), String>,
    /// Sleep-timer minutes input per device id (raw string).
    pub(crate) timer_input: HashMap<String, String>,
    /// Active timer state per device id.
    pub(crate) timers: HashMap<String, TimerState>,
    /// Active music sessions per device id (instant-control mode).
    pub(crate) music: HashMap<String, crate::message::MusicSession>,
    /// Active ambient sessions per device id (presence = running). Holds the live
    /// config sender (region/mode/targets) and the resolved sink.
    pub(crate) ambient: HashMap<String, AmbientRun>,
    /// Per-device ambient UI selections, persisted across tab switches even when stopped.
    pub(crate) ambient_cfg: HashMap<String, crate::ambient::AmbientConfig>,
    /// Cached display list for the ambient monitor picker. Enumerating spawns a subprocess
    /// (`hyprctl`/scap), so it's resolved at startup and on each scan, not per redraw.
    // ponytail: refreshed on scan, not hot-plug. Rescan to pick up a plugged/unplugged display.
    pub(crate) monitors: Vec<crate::ambient::capture::Monitor>,
    /// Last local interaction per device id; gates notification reconciliation
    /// (see [`SYNC_DEBOUNCE`]).
    pub(crate) last_input: HashMap<String, Instant>,
    /// Bounded in-memory command log shown on the logging screen (newest at back).
    pub(crate) logs: VecDeque<LogEntry>,
    /// Pause command-log capture (both the screen buffer and the file).
    pub(crate) log_paused: bool,
    /// Filter the log view to one device id (`None` = show all).
    pub(crate) log_filter: Option<String>,
    /// Append handle for the on-disk command log (`None` if it couldn't be opened).
    pub(crate) log_file: Option<File>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: crate::message::Screen::default(),
            devices: Vec::new(),
            selected: None,
            clients: HashMap::new(),
            pickers: HashMap::new(),
            force_all: false,
            settings_tab: SettingsTab::default(),
            theme_pref: crate::theme::default_pref(),
            theme: crate::theme::ember_dark(),
            timeout_secs: 3,
            status: Status::Idle,
            scanning: false,
            scan_progress: 0.0,
            inflight: HashSet::new(),
            asked_open: false,
            tabs: HashMap::new(),
            rename: None,
            flow_rows: HashMap::new(),
            flow_count: HashMap::new(),
            timer_input: HashMap::new(),
            timers: HashMap::new(),
            music: HashMap::new(),
            ambient: HashMap::new(),
            ambient_cfg: HashMap::new(),
            monitors: Vec::new(),
            last_input: HashMap::new(),
            logs: VecDeque::new(),
            log_paused: false,
            log_filter: None,
            log_file: None,
        }
    }
}

impl App {
    /// Initial state plus an immediate scan. Scanning needs no root — `ufw` filters
    /// packets, not `bind`/`send`, so the M-SEARCH goes out regardless. If the
    /// firewall is blocking inbound 1982, replies are dropped and the empty result
    /// (handled in [`Message::Scanned`]) is what offers to open the port.
    pub(crate) fn boot() -> (Self, Task<Message>) {
        let mut app = Self::default();
        // Restore persisted settings before the scan (timeout feeds start_scan).
        let s = crate::settings::load();
        app.force_all = s.force_all;
        app.timeout_secs = s.timeout_secs;
        app.theme = resolve_theme(&s.theme_pref);
        app.theme_pref = s.theme_pref;
        // Seed the device list from the cache for an instant UI; the scan below
        // refreshes IPs, opens live connections, and re-saves.
        app.devices = match yeelight_core::registry::load(crate::settings::devices_path()) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "device cache load failed");
                Vec::new()
            }
        };
        app.selected = (!app.devices.is_empty()).then_some(0);
        app.log_file = open_log_file();
        let task = app.start_scan();
        (app, task)
    }

    /// Persist the current settings (best-effort; see [`crate::settings`]).
    fn save_settings(&self) {
        crate::settings::save(&crate::settings::Settings {
            force_all: self.force_all,
            timeout_secs: self.timeout_secs,
            theme_pref: self.theme_pref.clone(),
        });
    }

    /// Kick off a LAN scan.
    fn start_scan(&mut self) -> Task<Message> {
        self.scanning = true;
        self.scan_progress = 0.0;
        self.status = Status::Ok("scanning…".into());
        // Refresh the display list while we're enumerating the LAN (cheap, user-initiated).
        self.monitors = crate::ambient::capture::monitors();
        let secs = self.timeout_secs.max(1) as u64;
        Task::perform(
            async move {
                discovery::search(Duration::from_secs(secs))
                    .await
                    .map_err(|e| e.to_string())
            },
            Message::Scanned,
        )
    }

    /// Whether the given control currently has a command in flight.
    pub(crate) fn btn_busy(&self, id: &str, bg: bool, btn: Btn) -> bool {
        self.inflight.contains(&(id.to_owned(), bg, btn))
    }

    /// The id of the currently selected device, if any.
    fn selected_id(&self) -> Option<String> {
        self.selected
            .and_then(|i| self.devices.get(i))
            .map(|d| d.id.clone())
    }

    /// Resolve a user intent against the selected device's capabilities, apply an
    /// optimistic local state update, and return the task that talks to the bulb.
    fn dispatch(&mut self, bg: bool, kind: CmdKind) -> Task<Message> {
        let Some(i) = self.selected else {
            return Task::none();
        };
        let id = self.devices[i].id.clone();
        self.last_input.insert(id.clone(), Instant::now());

        // Resolve BEFORE the optimistic mutation so SetPower reads the current power.
        let op = {
            let d = &self.devices[i];
            match kind {
                CmdKind::Toggle => {
                    let has_toggle = if bg {
                        d.supports("bg_toggle")
                    } else {
                        d.supports("toggle")
                    };
                    if has_toggle {
                        Op::Toggle(bg)
                    } else {
                        Op::SetPower(bg, !d.state.power.unwrap_or(false))
                    }
                }
                CmdKind::SetColor(c) => Op::SetRgb(bg, color_to_u32(c)),
                CmdKind::SetBright(b) => Op::SetBright(bg, b),
                CmdKind::SetTemp(ct) => Op::SetCt(bg, ct),
            }
        };

        // Optimistic local update (notifications are deferred; next scan corrects drift).
        // State has only main-light fields, so bg ops update nothing locally.
        let d = &mut self.devices[i];
        match op {
            Op::Toggle(false) => d.state.power = Some(!d.state.power.unwrap_or(false)),
            Op::SetPower(false, on) => d.state.power = Some(on),
            Op::SetRgb(false, rgb) => {
                d.state.rgb = Some(rgb);
                d.state.color_mode = Some(1);
            }
            Op::SetBright(false, b) => d.state.bright = Some(b),
            Op::SetCt(false, ct) => {
                d.state.ct = Some(ct);
                d.state.color_mode = Some(2);
            }
            // Background light has no local state mirror; the next scan reconciles it.
            Op::Toggle(true)
            | Op::SetPower(true, _)
            | Op::SetRgb(true, _)
            | Op::SetBright(true, _)
            | Op::SetCt(true, _) => {}
        }

        // Sync the picker draft so preset chips / preview highlight the chosen
        // value at once — the bg light has no local State to read back, and
        // chips send Command directly (not the *Draft messages the sliders use).
        {
            let p = self.pickers.entry(id.clone()).or_default();
            match op {
                // Picking a color/temp also flips the Light-tab segment to match.
                Op::SetRgb(true, rgb) => { p.bg_draft = u32_to_color(rgb); p.bg_seg = Some(false); }
                Op::SetRgb(false, rgb) => { p.main_draft = u32_to_color(rgb); p.main_seg = Some(false); }
                Op::SetCt(true, ct) => { p.bg_ct = ct; p.bg_seg = Some(true); }
                Op::SetCt(false, ct) => { p.main_ct = ct; p.main_seg = Some(true); }
                Op::SetBright(true, b) => p.bg_bright = b,
                Op::SetBright(false, b) => p.main_bright = b,
                Op::Toggle(_) | Op::SetPower(..) => {}
            }
        }

        let btn = match kind {
            CmdKind::Toggle => Btn::Toggle,
            CmdKind::SetColor(_) => Btn::Color,
            CmdKind::SetBright(_) => Btn::Bright,
            CmdKind::SetTemp(_) => Btn::Temp,
        };
        let key: OpKey = (id, bg, btn);
        self.inflight.insert(key.clone());

        // Instant mode: stream continuous controls (color/bright/temp) fire-and-forget
        // over the music channel — no rate limit, no reply. Power/toggle fall through
        // to the request/response client below.
        if let Some(session) = self.music.get(&key.0).cloned()
            && let Some((method, params)) = music_params(op)
        {
            self.inflight.remove(&key); // music sends don't await a reply
            return Task::perform(
                async move {
                    let mut s = session.lock().await;
                    s.send(method, params).await.map(|()| op.label()).map_err(|e| e.to_string())
                },
                move |result| Message::CommandDone { key: key.clone(), result },
            );
        }

        if let Some(client) = self.clients.get(&key.0) {
            run_task(Arc::clone(client), op, key)
        } else {
            let device = self.devices[i].clone();
            Task::perform(
                async move {
                    Client::connect(device)
                        .await
                        .map(Arc::new)
                        .map_err(|e| e.to_string())
                },
                move |client| Message::Connected {
                    key: key.clone(),
                    client,
                    op,
                },
            )
        }
    }

    /// The update loop.
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Scan => self.start_scan(),
            Message::PortPromptAnswered(false) => {
                self.status = Status::Ok("0 device(s) found".into());
                Task::none()
            }
            Message::PortPromptAnswered(true) => {
                self.status = Status::Ok("opening discovery port…".into());
                Task::perform(
                    async move {
                        firewall::ensure_udp_open_pkexec(discovery::SSDP_PORT)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::PortOpened,
                )
            }
            Message::PortOpened(res) => match res {
                Ok(()) => self.start_scan(),
                Err(e) => {
                    self.status = Status::Err(e);
                    Task::none()
                }
            },
            Message::Scanned(res) => {
                self.scanning = false;
                match res {
                    Ok(devices) => {
                        let n = devices.len();
                        self.devices = devices;
                        self.selected = (!self.devices.is_empty()).then_some(0);
                        self.status = Status::Ok(format!("{n} device(s) found"));
                        // Cache for next launch, but don't clobber a good cache
                        // with an empty result (a firewalled/transient scan finds
                        // nothing). ponytail: a stale cache only flashes old
                        // devices on boot — the next scan corrects it.
                        if !self.devices.is_empty()
                            && let Err(e) = yeelight_core::registry::save(
                                crate::settings::devices_path(),
                                &self.devices,
                            )
                        {
                            tracing::warn!(error = %e, "device cache save failed");
                        }
                        // Found nothing on the first empty scan? The firewall is the
                        // likely culprit (it drops inbound replies). Offer to open it
                        // once — declining or finding bulbs never nags again.
                        if self.devices.is_empty() && !self.asked_open {
                            self.asked_open = true;
                            return Task::perform(ask_open_port(), Message::PortPromptAnswered);
                        }
                        // Open a persistent control connection to each new device so we
                        // hear its push notifications (external changes from other apps /
                        // the physical switch). Without a live connection we listen to
                        // nothing — the device only pushes to connected control sockets.
                        let tasks: Vec<Task<Message>> = self
                            .devices
                            .iter()
                            .filter(|d| !self.clients.contains_key(&d.id))
                            .map(|d| {
                                let (device, id) = (d.clone(), d.id.clone());
                                Task::perform(
                                    async move {
                                        Client::connect(device).await.map(Arc::new).map_err(|e| e.to_string())
                                    },
                                    move |client| Message::Listening { id: id.clone(), client },
                                )
                            })
                            .collect();
                        return Task::batch(tasks);
                    }
                    Err(e) => self.status = Status::Err(e),
                }
                Task::none()
            }
            Message::SelectScreen(s) => { self.screen = s; Task::none() }
            Message::SelectTab(i) => {
                if i < self.devices.len() {
                    self.selected = Some(i);
                    self.screen = crate::message::Screen::Device;
                }
                Task::none()
            }
            Message::SelectDetailTab { bg, tab } => {
                if let Some(id) = self.selected_id() { self.tabs.insert((id, bg), tab); }
                Task::none()
            }
            Message::SetLightSeg { bg, temp } => {
                if let Some(id) = self.selected_id() {
                    self.last_input.insert(id.clone(), Instant::now());
                    let p = self.pickers.entry(id).or_default();
                    if bg { p.bg_seg = Some(temp); } else { p.main_seg = Some(temp); }
                }
                Task::none()
            }
            Message::RenameStart => {
                if let Some(d) = self.selected.and_then(|i| self.devices.get(i)) {
                    let name = d.state.name.clone().filter(|n| !n.is_empty())
                        .unwrap_or_else(|| d.model.clone().into());
                    self.rename = Some((d.id.clone(), name));
                }
                Task::none()
            }
            Message::RenameEdit(s) => {
                if let Some((_, buf)) = &mut self.rename { *buf = s; }
                Task::none()
            }
            Message::RenameCancel => { self.rename = None; Task::none() }
            Message::RenameCommit => self.rename_commit(),
            Message::ApplyScene { bg, index } => self.apply_scene(bg, index),
            Message::SaveDefault => self.run_selected("set_default", |c| async move { c.set_default().await }),
            Message::StopFlow { bg } => self.run_selected("stop_cf", move |c| async move {
                if bg { c.bg_stop_cf().await } else { c.stop_cf().await }
            }),
            Message::ApplyFlowPreset { bg, index } => self.apply_flow_preset(bg, index),
            Message::FlowRowAdd { bg } => {
                if let Some(i) = self.selected {
                    let d = &self.devices[i];
                    let force = self.force_all;
                    let has = |m: &str| force || d.supports(m);
                    // Default a new step to a mode the light supports: color if it
                    // can, else temperature (a temp-only light can't do color steps).
                    let temp_only = !has(if bg { "bg_set_rgb" } else { "set_rgb" })
                        && has(if bg { "bg_set_ct_abx" } else { "set_ct_abx" });
                    let id = d.id.clone();
                    let mut r = FlowRow::default();
                    if temp_only {
                        r.mode = 2;
                        r.value = "4000".into();
                    }
                    self.flow_rows.entry((id, bg)).or_default().push(r);
                }
                Task::none()
            }
            Message::FlowRowDel { bg, row } => {
                if let Some(id) = self.selected_id()
                    && let Some(rows) = self.flow_rows.get_mut(&(id, bg))
                    && row < rows.len()
                {
                    rows.remove(row);
                }
                Task::none()
            }
            Message::FlowRowEdit { bg, row, field, value } => {
                if let Some(id) = self.selected_id()
                    && let Some(rows) = self.flow_rows.get_mut(&(id, bg))
                    && let Some(r) = rows.get_mut(row)
                {
                    r.set(field, value);
                }
                Task::none()
            }
            Message::FlowCountEdit { bg, value } => {
                if let Some(id) = self.selected_id() { self.flow_count.insert((id, bg), value); }
                Task::none()
            }
            Message::StartCustomFlow { bg } => self.start_custom_flow(bg),
            Message::TimerEdit(s) => {
                if let Some(id) = self.selected_id() { self.timer_input.insert(id, s); }
                Task::none()
            }
            Message::TimerStart => self.timer_start(),
            Message::TimerCancel => self.timer_cancel(),
            Message::TimerTick => { self.tick_timers(); Task::none() }
            Message::MusicToggle => self.music_toggle(),
            Message::MusicStarted { id, session } => {
                match session {
                    Ok(s) => { self.music.insert(id, s); self.status = Status::Ok("instant mode on".into()); }
                    Err(e) => self.status = Status::Err(e),
                }
                Task::none()
            }
            Message::AmbientToggle => self.ambient_toggle(),
            Message::AmbientStarted { id, sink } => {
                match sink {
                    Ok(sink) => {
                        let cfg = self.ambient_cfg.get(&id).cloned().unwrap_or_default();
                        let (cfg_tx, _) = tokio::sync::watch::channel(cfg.clone());
                        // Register the music session (whether ambient reused or opened it) as
                        // the shared instant-mode session, so the Music tab reflects it and a
                        // manual toggle reuses it instead of opening a second connection.
                        if let Some(music) = &sink.music {
                            self.music.entry(id.clone()).or_insert_with(|| music.clone());
                        }
                        self.ambient.insert(
                            id,
                            AmbientRun { cfg_tx, sink, monitor_id: cfg.monitor_id },
                        );
                        self.status = Status::Ok("ambient on".into());
                    }
                    Err(e) => {
                        self.status = Status::Err(format!("ambient: {e}"));
                    }
                }
                Task::none()
            }
            Message::AmbientSetRegion(region) => self.ambient_edit(|c| c.region = region),
            Message::AmbientSetMode(mode) => self.ambient_edit(|c| c.mode = mode),
            Message::AmbientSetTarget { main, on } => {
                self.ambient_edit(|c| if main { c.main = on } else { c.bg = on })
            }
            Message::AmbientSetMonitor(monitor_id) => {
                // Monitor is fixed while running; only takes effect on next start.
                if let Some(id) = self.selected_id() {
                    self.ambient_cfg.entry(id).or_default().monitor_id = monitor_id;
                }
                Task::none()
            }
            Message::AmbientError { id, error } => {
                tracing::warn!(%id, %error, "ambient send failed");
                self.status = Status::Err(format!("ambient: {error}"));
                Task::none()
            }
            Message::Listening { id, client } => {
                // Offline/unreachable devices are skipped: the tab still works, it
                // just won't show live external changes. or_insert avoids clobbering
                // a client a concurrent command already cached.
                match client {
                    Ok(arc) => {
                        tracing::info!(%id, "listening: control connection established");
                        arc.set_force(self.force_all);
                        self.clients.entry(id).or_insert(arc);
                    }
                    Err(e) => tracing::warn!(%id, error = %e, "listening: connect failed"),
                }
                Task::none()
            }
            Message::Command { bg, kind } => self.dispatch(bg, kind),
            Message::Connected { key, client, op } => match client {
                Ok(arc) => {
                    arc.set_force(self.force_all);
                    self.clients.insert(key.0.clone(), Arc::clone(&arc));
                    run_task(arc, op, key)
                }
                Err(e) => {
                    self.inflight.remove(&key);
                    self.status = Status::Err(e);
                    Task::none()
                }
            },
            Message::CommandDone { key, result } => {
                self.inflight.remove(&key);
                self.status = match result {
                    Ok(label) => Status::Ok(label),
                    Err(e) => Status::Err(e),
                };
                Task::none()
            }
            Message::OpenPicker { bg } => {
                if let Some(id) = self.selected_id() {
                    let p = self.pickers.entry(id).or_default();
                    if bg {
                        p.bg_open = true;
                    } else {
                        p.main_open = true;
                    }
                }
                Task::none()
            }
            Message::CancelPicker { bg } => {
                if let Some(id) = self.selected_id() {
                    let p = self.pickers.entry(id).or_default();
                    if bg {
                        p.bg_open = false;
                    } else {
                        p.main_open = false;
                    }
                }
                Task::none()
            }
            Message::PickColor { bg, color } => {
                if let Some(id) = self.selected_id() {
                    let p = self.pickers.entry(id).or_default();
                    if bg {
                        p.bg_open = false;
                        p.bg_draft = color;
                    } else {
                        p.main_open = false;
                        p.main_draft = color;
                    }
                }
                self.dispatch(bg, CmdKind::SetColor(color))
            }
            Message::BrightDraft { bg, value } => {
                if let Some(id) = self.selected_id() {
                    self.last_input.insert(id.clone(), Instant::now());
                    let p = self.pickers.entry(id).or_default();
                    if bg {
                        p.bg_bright = value;
                    } else {
                        p.main_bright = value;
                    }
                }
                Task::none()
            }
            Message::TempDraft { bg, value } => {
                if let Some(id) = self.selected_id() {
                    self.last_input.insert(id.clone(), Instant::now());
                    let p = self.pickers.entry(id).or_default();
                    if bg {
                        p.bg_ct = value;
                    } else {
                        p.main_ct = value;
                    }
                }
                Task::none()
            }
            Message::ForceAllToggled(on) => {
                self.force_all = on;
                for c in self.clients.values() {
                    c.set_force(on);
                }
                self.save_settings();
                Task::none()
            }
            Message::SelectSettingsTab(tab) => {
                self.settings_tab = tab;
                Task::none()
            }
            Message::ThemeChanged(pref) => {
                self.theme = resolve_theme(&pref);
                self.theme_pref = pref;
                self.save_settings();
                Task::none()
            }
            Message::TimeoutChanged(s) => {
                let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                self.timeout_secs = digits.parse().unwrap_or(0);
                // ponytail: writes per keystroke; the file is a few bytes, so no
                // debounce. Add one if this ever shows up on a profile.
                self.save_settings();
                Task::none()
            }
            Message::Tick => {
                if self.scanning {
                    let step = 0.1 / self.timeout_secs.max(1) as f32;
                    self.scan_progress = (self.scan_progress + step).min(1.0);
                }
                Task::none()
            }
            Message::StateChanged { id, params } => {
                // Reconcile the controls from device truth, unless the user just
                // touched this device (debounce) — otherwise a notification mid-drag,
                // or the device echoing our own command, would yank the sliders. Ambient
                // streams color several times/sec, so suppress reconciliation entirely
                // while it runs, else its echoes would yank the pickers continuously.
                let fresh = !self.ambient.contains_key(&id)
                    && self.last_input.get(&id).is_none_or(|t| t.elapsed() >= SYNC_DEBOUNCE);
                tracing::debug!(%id, ?params, fresh, "props notification");
                let Some(d) = self.devices.iter_mut().find(|d| d.id == id) else {
                    return Task::none();
                };
                apply_props(&mut d.state, &params);
                if fresh {
                    let s = &d.state;
                    let p = self.pickers.entry(id).or_default();
                    // Main light.
                    if let Some(b) = s.bright { p.main_bright = b; }
                    if let Some(ct) = s.ct { p.main_ct = ct; }
                    if let Some(rgb) = s.rgb { p.main_draft = u32_to_color(rgb); }
                    if let Some(mode) = s.color_mode { p.main_seg = Some(mode == 2); }
                    // Background light.
                    if let Some(b) = s.bg_bright { p.bg_bright = b; }
                    if let Some(ct) = s.bg_ct { p.bg_ct = ct; }
                    if let Some(rgb) = s.bg_rgb { p.bg_draft = u32_to_color(rgb); }
                    if let Some(mode) = s.bg_color_mode { p.bg_seg = Some(mode == 2); }
                }
                Task::none()
            }
            Message::Logged { id, direction, line } => {
                if !self.log_paused {
                    let entry = LogEntry { time: SystemTime::now(), device: id, direction, line };
                    if let Some(f) = &mut self.log_file {
                        let arrow = match entry.direction {
                            Direction::Sent => "->",
                            Direction::Received => "<-",
                        };
                        if let Err(e) = writeln!(
                            f,
                            "{} UTC {arrow} {} {}",
                            fmt_time(entry.time), entry.device, entry.line,
                        ) {
                            tracing::warn!(error = %e, "log file: write failed");
                        }
                    }
                    self.logs.push_back(entry);
                    while self.logs.len() > LOG_CAP {
                        self.logs.pop_front();
                    }
                }
                Task::none()
            }
            Message::LogClear => { self.logs.clear(); Task::none() }
            Message::LogTogglePause => { self.log_paused = !self.log_paused; Task::none() }
            Message::LogOpenFile => {
                // open::that_detached picks the platform opener (xdg-open / open / start)
                // and returns immediately, so the UI thread never blocks.
                if let Err(e) = open::that_detached(crate::settings::log_path()) {
                    self.status = Status::Err(format!("open log: {e}"));
                }
                Task::none()
            }
            Message::LogFilterDevice(f) => { self.log_filter = f; Task::none() }
        }
    }

    /// The view.
    pub(crate) fn view(&self) -> Element<'_, Message> {
        view::root(self)
    }

    /// The active iced theme (resolved from [`App::theme_mode`]).
    pub(crate) fn theme(&self) -> iced::Theme {
        self.theme.clone()
    }

    /// The active detail tab for the selected device's main or background light.
    pub(crate) fn active_tab(&self, bg: bool) -> crate::message::DetailTab {
        self.selected_id()
            .and_then(|id| self.tabs.get(&(id, bg)).copied())
            .unwrap_or_default()
    }

    /// Commit the in-progress device rename: mutate local state and send `set_name`.
    fn rename_commit(&mut self) -> Task<Message> {
        let Some((id, name)) = self.rename.take() else { return Task::none() };
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
            d.state.name = Some(name.clone());
        }
        self.run_selected("name set", move |c| async move { c.set_name(&name).await })
    }

    /// Apply a preset scene (by index into [`crate::presets::SCENES`]) to the
    /// given light.
    fn apply_scene(&mut self, bg: bool, i: usize) -> Task<Message> {
        let Some(p) = crate::presets::SCENES.get(i) else { return Task::none() };
        let scene = (p.make)();
        self.run_selected("scene applied", move |c| async move {
            if bg { c.bg_set_scene(scene).await } else { c.set_scene(scene).await }
        })
    }
    /// Apply a preset flow (by index into [`crate::presets::FLOWS`]) to the given light.
    fn apply_flow_preset(&mut self, bg: bool, i: usize) -> Task<Message> {
        let Some(p) = crate::presets::FLOWS.get(i) else { return Task::none() };
        let (count, action, expr) = (p.count, p.action, (p.make)());
        self.run_selected("flow started", move |c| async move {
            if bg { c.bg_start_cf(count, action, expr).await } else { c.start_cf(count, action, expr).await }
        })
    }

    /// Start the custom flow from the editor draft. Validates the expression
    /// locally (non-empty, each step >=50ms) before sending so a bad draft
    /// surfaces in the status bar instead of a device rejection.
    fn start_custom_flow(&mut self, bg: bool) -> Task<Message> {
        let Some(id) = self.selected_id() else { return Task::none() };
        let key = (id, bg);
        let rows = self.flow_rows.get(&key).cloned().unwrap_or_default();
        let count = self.flow_count.get(&key).and_then(|s| s.parse().ok()).unwrap_or(0u32);
        let expr = match rows_to_expr(&rows) {
            Ok(e) => e,
            Err(e) => {
                self.status = Status::Err(e);
                return Task::none();
            }
        };
        if let Err(e) = expr.render() {
            self.status = Status::Err(e.to_string());
            return Task::none();
        }
        self.run_selected("flow started", move |c| async move {
            if bg {
                c.bg_start_cf(count, FlowAction::Recover, expr).await
            } else {
                c.start_cf(count, FlowAction::Recover, expr).await
            }
        })
    }
    /// Start a sleep timer on the selected device: power off after the entered
    /// minutes. Seeds a local countdown the [`Message::TimerTick`] subscription drives.
    fn timer_start(&mut self) -> Task<Message> {
        let Some(id) = self.selected_id() else { return Task::none() };
        let mins: u32 = self.timer_input.get(&id).and_then(|s| s.parse().ok()).unwrap_or(0);
        if mins == 0 {
            self.status = Status::Err("enter minutes > 0".into());
            return Task::none();
        }
        self.timers.insert(id, TimerState { remaining: Some(mins * 60) });
        self.run_selected("timer started", move |c| async move {
            c.cron_add(CronType::PowerOff, mins).await
        })
    }

    /// Cancel the selected device's sleep timer.
    fn timer_cancel(&mut self) -> Task<Message> {
        if let Some(id) = self.selected_id() {
            self.timers.remove(&id);
        }
        self.run_selected("timer cancelled", |c| async move { c.cron_del(CronType::PowerOff).await })
    }

    /// Advance all active countdowns by one second, dropping any that hit zero.
    fn tick_timers(&mut self) {
        for t in self.timers.values_mut() {
            if let Some(r) = t.remaining {
                t.remaining = Some(r.saturating_sub(1)).filter(|&x| x > 0);
            }
        }
        self.timers.retain(|_, t| t.remaining.is_some());
    }
    /// Toggle music "instant control" mode for the selected device. Off → on opens
    /// a [`MusicConnection`] (requires an existing client); on → off disables it.
    fn music_toggle(&mut self) -> Task<Message> {
        let Some(i) = self.selected else { return Task::none() };
        let id = self.devices[i].id.clone();

        // Already on → turn off: drop the session and disable music mode on the device.
        if self.music.remove(&id).is_some() {
            self.status = Status::Ok("instant mode off".into());
            return self.run_selected("instant mode off", |c| async move { c.set_music_off().await });
        }

        // Turn on: needs a connected client to start the reverse channel.
        let Some(client) = self.clients.get(&id).cloned() else {
            self.status = Status::Err("connect first (use a control), then enable instant mode".into());
            return Task::none();
        };
        self.status = Status::Ok("starting instant mode…".into());
        Task::perform(
            async move {
                MusicConnection::start(&client, DEFAULT_MUSIC_PORT)
                    .await
                    .map(|m| Arc::new(tokio::sync::Mutex::new(m)))
                    .map_err(|e| e.to_string())
            },
            move |session| Message::MusicStarted { id: id.clone(), session },
        )
    }

    /// Apply an edit to the selected device's ambient config and, if running, push it
    /// live to the driver (no restart). Persists to `ambient_cfg` either way.
    fn ambient_edit(&mut self, f: impl FnOnce(&mut crate::ambient::AmbientConfig)) -> Task<Message> {
        let Some(id) = self.selected_id() else { return Task::none() };
        let cfg = self.ambient_cfg.entry(id.clone()).or_default();
        f(cfg);
        let updated = cfg.clone();
        if let Some(run) = self.ambient.get(&id) {
            let _ = run.cfg_tx.send(updated); // live reconfigure
        }
        Task::none()
    }

    /// Start or stop ambient mode for the selected device.
    fn ambient_toggle(&mut self) -> Task<Message> {
        let Some(id) = self.selected_id() else { return Task::none() };

        // Running → stop: drop the session (subscription ends → capture stops). Any music
        // session is left up as the shared instant-mode session — tearing it down here would
        // kill instant mode if the Music tab is also using it. The user turns it off there.
        if self.ambient.remove(&id).is_some() {
            self.status = Status::Ok("ambient off".into());
            return Task::none();
        }

        // Stopped → start. Need a connected client (like music_toggle).
        let Some(client) = self.clients.get(&id).cloned() else {
            self.status =
                Status::Err("connect first (use a control), then enable ambient".into());
            return Task::none();
        };

        // Resolve capability booleans up front (owned) so later `self.status = ..` mutations
        // don't conflict with a borrow of `self.devices`.
        let (has_music, caps) = {
            let device = &self.devices[self.selected.unwrap()];
            let has = |m: &str| self.force_all || device.supports(m);
            (
                has("set_music"),
                crate::ambient::Caps {
                    main_rgb: has("set_rgb"),
                    main_ct: has("set_ct_abx"),
                    bg_rgb: has("bg_set_rgb"),
                    bg_ct: has("bg_set_ct_abx"),
                },
            )
        };
        // Ambient drives any color-capable target — rgb or temperature.
        let has_color = caps.main_rgb || caps.main_ct || caps.bg_rgb || caps.bg_ct;

        // Seed a default config (so the UI and the AmbientStarted handler agree) with targets
        // reconciled to what this bulb can actually color — preferring main, else bg. Only on
        // first creation, so it never clobbers a returning user's explicit target choices.
        self.ambient_cfg.entry(id.clone()).or_insert_with(|| {
            let (main, bg) = crate::ambient::default_targets(caps);
            crate::ambient::AmbientConfig { main, bg, ..Default::default() }
        });

        // Prefer an existing music session; else start one if supported; else go direct.
        if let Some(music) = self.music.get(&id).cloned() {
            let sink = crate::ambient::AmbientSink { client, music: Some(music), caps };
            self.status = Status::Ok("ambient on (music)".into());
            return Task::done(Message::AmbientStarted { id, sink: Ok(sink) });
        }

        if has_music {
            self.status = Status::Ok("ambient: starting music…".into());
            let c = Arc::clone(&client);
            return Task::perform(
                async move {
                    MusicConnection::start(&c, DEFAULT_MUSIC_PORT)
                        .await
                        .map(|m| Arc::new(tokio::sync::Mutex::new(m)))
                        .map_err(|e| e.to_string())
                },
                move |res| match res {
                    Ok(music) => Message::AmbientStarted {
                        id: id.clone(),
                        sink: Ok(crate::ambient::AmbientSink {
                            client: Arc::clone(&client),
                            music: Some(music),
                            caps,
                        }),
                    },
                    // Music handshake failed: degrade to the direct (rate-limited) sink if the
                    // bulb has any color control, rather than failing the whole feature (spec).
                    Err(_) if has_color => Message::AmbientStarted {
                        id: id.clone(),
                        sink: Ok(crate::ambient::AmbientSink {
                            client: Arc::clone(&client),
                            music: None,
                            caps,
                        }),
                    },
                    Err(e) => Message::AmbientStarted { id: id.clone(), sink: Err(e) },
                },
            );
        }

        // Direct fallback — needs at least one color-capable target.
        if has_color {
            let sink = crate::ambient::AmbientSink { client, music: None, caps };
            self.status = Status::Ok("ambient on (fallback 2fps)".into());
            return Task::done(Message::AmbientStarted { id, sink: Ok(sink) });
        }

        self.status = Status::Err("device has no color control for ambient".into());
        Task::none()
    }

    /// Run a one-shot async operation against the selected device's client.
    ///
    /// If a cached client exists it is reused; otherwise a temporary connection is
    /// opened for the duration of the call.
    fn run_selected<F, Fut>(&mut self, label: &'static str, f: F) -> Task<Message>
    where
        F: FnOnce(Arc<Client>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = yeelight_core::Result<()>> + Send + 'static,
    {
        let Some(i) = self.selected else { return Task::none() };
        let id = self.devices[i].id.clone();
        if let Some(client) = self.clients.get(&id).cloned() {
            Task::perform(
                async move {
                    f(client).await.map(|()| label.to_string()).map_err(|e| e.to_string())
                },
                move |result| Message::CommandDone { key: (id.clone(), false, Btn::Misc), result },
            )
        } else {
            let device = self.devices[i].clone();
            let force = self.force_all;
            // ponytail: the freshly connected client is not inserted into self.clients
            // because we're in an async closure with no access to &mut self. These
            // one-shot actions are infrequent; color/bright/temp go through dispatch()
            // which does cache via Message::Connected.
            Task::perform(
                async move {
                    let c = Client::connect(device).await.map(Arc::new).map_err(|e| e.to_string())?;
                    c.set_force(force);
                    f(Arc::clone(&c)).await.map(|()| label.to_string()).map_err(|e| e.to_string())
                },
                move |result| Message::CommandDone { key: (id.clone(), false, Btn::Misc), result },
            )
        }
    }

    /// One live-notification stream per connected device; iced keys them by id and
    /// keeps each running until its client disappears.
    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let mut subs: Vec<Subscription<Message>> = self
            .clients
            .iter()
            .map(|(id, client)| {
                Subscription::run_with(
                    Sub {
                        id: id.clone(),
                        client: Arc::clone(client),
                    },
                    build_stream,
                )
            })
            .collect();
        // A second stream per device for the raw command log (distinct recipe type
        // from `Sub`, so a device runs both a notification and a log stream).
        for (id, client) in &self.clients {
            subs.push(Subscription::run_with(
                LogSub { id: id.clone(), client: Arc::clone(client) },
                build_log_stream,
            ));
        }
        for (id, run) in &self.ambient {
            subs.push(Subscription::run_with(
                AmbientSub {
                    id: id.clone(),
                    monitor_id: run.monitor_id,
                    sink: run.sink.clone(),
                    cfg_rx: run.cfg_tx.subscribe(),
                },
                build_ambient_stream,
            ));
        }
        if self.scanning {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick));
        }
        if !self.timers.is_empty() {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::TimerTick));
        }
        Subscription::batch(subs)
    }
}

/// Subscription key + source. Hashed by `id` so iced treats one device as one stream.
struct Sub {
    id: String,
    client: Arc<Client>,
}

impl Hash for Sub {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Turn a device's broadcast receiver into a stream of [`Message::StateChanged`].
/// A plain `fn` (not a closure) because iced's `run_with` takes a function pointer;
/// boxed so the return type doesn't borrow the input reference.
fn build_stream(sub: &Sub) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    let id = sub.id.clone();
    tracing::debug!(%id, "notification stream started");
    let stream = BroadcastStream::new(sub.client.notifications()).filter_map(move |res| {
        res.ok().map(|n| Message::StateChanged {
            id: id.clone(),
            params: n.params,
        })
    });
    Box::pin(stream)
}

/// Subscription key + source for the raw command log. A distinct type from [`Sub`]
/// so iced keeps a device's log stream separate from its notification stream.
struct LogSub {
    id: String,
    client: Arc<Client>,
}

impl Hash for LogSub {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Turn a device's log receiver into a stream of [`Message::Logged`].
fn build_log_stream(sub: &LogSub) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    let id = sub.id.clone();
    let stream = BroadcastStream::new(sub.client.logs()).filter_map(move |res| {
        res.ok().map(|l| Message::Logged {
            id: id.clone(),
            direction: l.direction,
            line: l.line,
        })
    });
    Box::pin(stream)
}

/// Subscription key + source for one device's ambient driver. Hashed by
/// `(id, monitor_id)` so region/mode/target edits (pushed via the cfg channel) do not
/// restart capture, but switching monitor does.
struct AmbientSub {
    id: String,
    monitor_id: Option<u32>,
    sink: crate::ambient::AmbientSink,
    cfg_rx: tokio::sync::watch::Receiver<crate::ambient::AmbientConfig>,
}

impl Hash for AmbientSub {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.monitor_id.hash(state);
    }
}

/// Build the ambient driver stream for a device (delegates to the ambient module).
fn build_ambient_stream(sub: &AmbientSub) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    crate::ambient::run_stream(sub.id.clone(), sub.sink.clone(), sub.cfg_rx.clone())
}

/// Open (creating parent dirs) the command log for appending. Best-effort: any
/// failure is logged and logging falls back to the in-memory screen only.
fn open_log_file() -> Option<File> {
    let path = crate::settings::log_path();
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(error = %e, "log file: mkdir failed");
        return None;
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(error = %e, "log file: open failed");
            None
        }
    }
}

/// Format a wall-clock time as `HH:MM:SS.mmm` (UTC).
// ponytail: UTC, no tz crate. Add chrono/time if local time matters for the log.
pub(crate) fn fmt_time(t: SystemTime) -> String {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        d.subsec_millis(),
    )
}

/// Merge a `props` notification (all string values) into the local [`State`].
fn apply_props(state: &mut State, params: &HashMap<String, String>) {
    for (k, v) in params {
        match k.as_str() {
            // Main light. Multi-light devices report `main_power`; single-light `power`.
            "power" | "main_power" => state.power = Some(v == "on"),
            "bright" => state.bright = v.parse().ok().or(state.bright),
            "rgb" => state.rgb = v.parse().ok().or(state.rgb),
            "ct" => state.ct = v.parse().ok().or(state.ct),
            "hue" => state.hue = v.parse().ok().or(state.hue),
            "sat" => state.sat = v.parse().ok().or(state.sat),
            "color_mode" => state.color_mode = v.parse().ok().or(state.color_mode),
            "name" => state.name = Some(v.clone()),
            // Background light.
            "bg_power" => state.bg_power = Some(v == "on"),
            "bg_bright" => state.bg_bright = v.parse().ok().or(state.bg_bright),
            "bg_rgb" => state.bg_rgb = v.parse().ok().or(state.bg_rgb),
            "bg_ct" => state.bg_ct = v.parse().ok().or(state.bg_ct),
            "bg_hue" => state.bg_hue = v.parse().ok().or(state.bg_hue),
            "bg_sat" => state.bg_sat = v.parse().ok().or(state.bg_sat),
            "bg_lmode" => state.bg_color_mode = v.parse().ok().or(state.bg_color_mode),
            _ => {}
        }
    }
}

/// Show a native yes/no popup offering to open the discovery port after an empty
/// scan. Returns `true` if the user agreed. On yes, the privileged `ufw allow` runs
/// via `pkexec`, so the system's own polkit dialog collects the password — no terminal.
async fn ask_open_port() -> bool {
    rfd::AsyncMessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Yeelight Studio")
        .set_description(format!(
            "No bulbs found. The firewall may be blocking discovery port {}/udp.\n\n\
             Open it now? Your system will ask for your password.",
            discovery::SSDP_PORT
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        .await
        == rfd::MessageDialogResult::Yes
}

/// Map a streamed [`Op`] to a `(method, params)` for [`MusicConnection::send`].
/// Returns `None` for ops that should not stream (power/toggle still go through the
/// request/response client). Uses a `sudden` transition — streaming wants instant steps.
fn music_params(op: Op) -> Option<(&'static str, Vec<Value>)> {
    let with_sudden = |v: u32, method: &'static str| {
        (method, vec![json!(v), json!("sudden"), json!(0)])
    };
    match op {
        Op::SetRgb(false, rgb) => Some(with_sudden(rgb, "set_rgb")),
        Op::SetRgb(true, rgb) => Some(with_sudden(rgb, "bg_set_rgb")),
        Op::SetBright(false, b) => Some(with_sudden(b as u32, "set_bright")),
        Op::SetBright(true, b) => Some(with_sudden(b as u32, "bg_set_bright")),
        Op::SetCt(false, ct) => Some(with_sudden(ct as u32, "set_ct_abx")),
        Op::SetCt(true, ct) => Some(with_sudden(ct as u32, "bg_set_ct_abx")),
        Op::Toggle(_) | Op::SetPower(..) => None,
    }
}

/// Build the async task that sends one resolved [`Op`] to the bulb.
fn run_task(client: Arc<Client>, op: Op, key: OpKey) -> Task<Message> {
    Task::perform(
        async move {
            let res = match op {
                Op::Toggle(true) => client.bg_toggle().await,
                Op::Toggle(false) => client.toggle().await,
                Op::SetPower(true, on) => client.bg_set_power(on, EFFECT, None).await,
                Op::SetPower(false, on) => client.set_power(on, EFFECT, None).await,
                Op::SetRgb(true, rgb) => client.bg_set_rgb(rgb, EFFECT).await,
                Op::SetRgb(false, rgb) => client.set_rgb(rgb, EFFECT).await,
                Op::SetBright(true, b) => client.bg_set_bright(b, EFFECT).await,
                Op::SetBright(false, b) => client.set_bright(b, EFFECT).await,
                Op::SetCt(true, ct) => client.bg_set_ct_abx(ct, EFFECT).await,
                Op::SetCt(false, ct) => client.set_ct_abx(ct, EFFECT).await,
            };
            res.map(|()| op.label()).map_err(|e| e.to_string())
        },
        move |result| Message::CommandDone {
            key: key.clone(),
            result,
        },
    )
}

/// Resolve a [`ThemePref`] to a concrete iced theme. `System` queries the OS once;
/// an unspecified or failed detection falls back to dark.
// ponytail: resolved at call time, not live-polled. Add a subscription that
// re-detects if following OS theme changes mid-session ever matters.
fn resolve_theme(pref: &ThemePref) -> iced::Theme {
    match pref {
        ThemePref::Fixed(theme) => theme.clone(),
        ThemePref::System => match dark_light::detect() {
            Ok(dark_light::Mode::Light) => iced::Theme::Light,
            _ => iced::Theme::Dark,
        },
    }
}

/// iced [`Color`] (f32 RGBA) → `0xRRGGBB`.
pub(crate) fn color_to_u32(c: Color) -> u32 {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to(c.r) << 16) | (to(c.g) << 8) | to(c.b)
}

/// `0xRRGGBB` → iced [`Color`].
pub(crate) fn u32_to_color(rgb: u32) -> Color {
    Color::from_rgb8(
        ((rgb >> 16) & 0xFF) as u8,
        ((rgb >> 8) & 0xFF) as u8,
        (rgb & 0xFF) as u8,
    )
}

/// Color temperature in Kelvin → an approximate display [`Color`] (Tanner
/// Helland's blackbody fit). Used only to tint the preview swatch in CT mode;
/// not sent to the device. Accurate enough over the 1700–6500K bulb range.
pub(crate) fn ct_to_color(kelvin: u16) -> Color {
    let t = kelvin as f32 / 100.0;
    let red = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let green = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_85)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    let ch = |v: f32| v.clamp(0.0, 255.0) / 255.0;
    Color::from_rgb(ch(red), ch(green), ch(blue))
}

/// Convert custom-flow editor rows to a [`FlowExpr`]. Unparseable fields fall
/// back to safe defaults (`0`, or `-1` "keep brightness"); the device-side
/// rules (each step >=50ms etc.) are enforced later by [`FlowExpr::render`].
fn rows_to_expr(rows: &[FlowRow]) -> Result<FlowExpr, String> {
    if rows.is_empty() {
        return Err("flow has no steps".into());
    }
    let tuples = rows
        .iter()
        .map(|r| FlowTuple {
            duration: r.duration.parse().unwrap_or(0),
            mode: r.mode,
            value: r.value.parse().unwrap_or(0),
            brightness: r.bright.parse().unwrap_or(-1),
        })
        .collect();
    Ok(FlowExpr(tuples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_time_formats_epoch() {
        assert_eq!(fmt_time(SystemTime::UNIX_EPOCH), "00:00:00.000");
        assert_eq!(fmt_time(SystemTime::UNIX_EPOCH + Duration::from_millis(3_661_500)), "01:01:01.500");
    }

    #[test]
    fn color_u32_round_trips() {
        for rgb in [0x000000, 0xFF0000, 0x00FF00, 0x0000FF, 0x123456, 0xFFFFFF] {
            assert_eq!(color_to_u32(u32_to_color(rgb)), rgb);
        }
    }

    #[test]
    fn ct_color_is_warm_low_cool_high() {
        let warm = ct_to_color(1700);
        let cool = ct_to_color(6500);
        // Warm = reddish (more red than blue); cool ≈ white (blue caught up).
        assert!(warm.r > warm.b);
        assert!(cool.b > warm.b);
        assert!(cool.r >= cool.b - 0.2); // near-neutral at the top
    }

    #[test]
    fn flow_rows_build_valid_expr() {
        let rows = vec![
            FlowRow { duration: "1000".into(), mode: 1, value: "16711680".into(), bright: "100".into() },
            FlowRow { duration: "500".into(), mode: 2, value: "2700".into(), bright: "50".into() },
        ];
        let expr = rows_to_expr(&rows).unwrap();
        assert_eq!(expr.render().unwrap(), "1000,1,16711680,100,500,2,2700,50");
    }

    #[test]
    fn flow_rows_reject_short_duration() {
        // Builds, but render() enforces the >=50ms rule from core.
        let rows = vec![FlowRow { duration: "10".into(), mode: 1, value: "1".into(), bright: "1".into() }];
        assert!(rows_to_expr(&rows).unwrap().render().is_err());
    }
}
