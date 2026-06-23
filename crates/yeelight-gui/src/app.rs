//! Application state, the update loop, and the async command plumbing.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use iced::{Color, Element, Subscription, Task};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use serde_json::{Value, json};
use yeelight_core::{
    Client, CronType, DEFAULT_MUSIC_PORT, Effect, FlowAction, FlowExpr, FlowTuple, MusicConnection,
    State, discovery, firewall,
};

use crate::message::{Btn, CmdKind, FlowField, Message, Op, OpKey, SettingsTab, ThemePref};
use crate::view;

/// Transition used for every GUI-issued command.
pub(crate) const EFFECT: Effect = Effect::Smooth(500);

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
    /// The discovery port is known-open (from the startup check or after opening it).
    pub(crate) port_open: bool,
    /// Active control tab per device id.
    pub(crate) tabs: HashMap<String, crate::message::DetailTab>,
    /// Which light (main/bg) controls target, per device id.
    pub(crate) target: HashMap<String, crate::message::Light>,
    /// In-progress rename buffer (device id, new name), if editing.
    pub(crate) rename: Option<(String, String)>,
    /// Custom flow-editor draft rows per device id.
    pub(crate) flow_rows: HashMap<String, Vec<FlowRow>>,
    /// Custom flow repeat-count input per device id (raw string).
    pub(crate) flow_count: HashMap<String, String>,
    /// Sleep-timer minutes input per device id (raw string).
    pub(crate) timer_input: HashMap<String, String>,
    /// Active timer state per device id.
    pub(crate) timers: HashMap<String, TimerState>,
    /// Active music sessions per device id (instant-control mode).
    pub(crate) music: HashMap<String, crate::message::MusicSession>,
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
            theme_pref: ThemePref::default(),
            theme: resolve_theme(&ThemePref::default()),
            timeout_secs: 3,
            status: Status::Idle,
            scanning: false,
            scan_progress: 0.0,
            inflight: HashSet::new(),
            port_open: false,
            tabs: HashMap::new(),
            target: HashMap::new(),
            rename: None,
            flow_rows: HashMap::new(),
            flow_count: HashMap::new(),
            timer_input: HashMap::new(),
            timers: HashMap::new(),
            music: HashMap::new(),
        }
    }
}

impl App {
    /// Initial state plus the startup firewall check. If the discovery port is
    /// already open the [`Message::PortChecked`] handler auto-starts a scan;
    /// otherwise the app idles until the user presses Scan.
    pub(crate) fn boot() -> (Self, Task<Message>) {
        let task = Task::perform(
            async move {
                firewall::is_udp_open(discovery::SSDP_PORT)
                    .await
                    .map_err(|e| e.to_string())
            },
            Message::PortChecked,
        );
        (Self::default(), task)
    }

    /// Kick off a LAN scan. Assumes the discovery port is open (caller gates on
    /// [`App::port_open`]).
    fn start_scan(&mut self) -> Task<Message> {
        self.scanning = true;
        self.scan_progress = 0.0;
        self.status = Status::Ok("scanning…".into());
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
            Op::Toggle(_) => d.state.power = Some(!d.state.power.unwrap_or(false)),
            Op::SetPower(_, on) => d.state.power = Some(on),
            Op::SetRgb(false, rgb) => {
                d.state.rgb = Some(rgb);
                d.state.color_mode = Some(1);
            }
            Op::SetBright(false, b) => d.state.bright = Some(b),
            Op::SetCt(false, ct) => {
                d.state.ct = Some(ct);
                d.state.color_mode = Some(2);
            }
            Op::SetRgb(true, _) | Op::SetBright(true, _) | Op::SetCt(true, _) => {}
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
            Message::Scan => {
                if self.port_open {
                    self.start_scan()
                } else {
                    // ponytail: port_open is cached from the startup check; if it
                    // went stale the idempotent sudo open below recovers. Not worth
                    // an async re-check on every press.
                    Task::perform(ask_open_port(), Message::PortPromptAnswered)
                }
            }
            Message::PortChecked(res) => match res {
                Ok(true) => {
                    self.port_open = true;
                    self.start_scan()
                }
                Ok(false) => {
                    self.status =
                        Status::Ok("discovery port closed — press Scan to open it".into());
                    Task::none()
                }
                Err(e) => {
                    self.status = Status::Err(e);
                    Task::none()
                }
            },
            Message::PortPromptAnswered(false) => {
                self.status =
                    Status::Ok("discovery port closed — press Scan to open it".into());
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
                Ok(()) => {
                    self.port_open = true;
                    self.start_scan()
                }
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
                    }
                    Err(e) => self.status = Status::Err(e),
                }
                Task::none()
            }
            Message::Quit => iced::exit(),
            Message::SelectScreen(s) => { self.screen = s; Task::none() }
            Message::SelectTab(i) => {
                if i < self.devices.len() {
                    self.selected = Some(i);
                    self.screen = crate::message::Screen::Device;
                }
                Task::none()
            }
            Message::SelectDetailTab(t) => {
                if let Some(id) = self.selected_id() { self.tabs.insert(id, t); }
                Task::none()
            }
            Message::SelectLight(l) => {
                if let Some(id) = self.selected_id() { self.target.insert(id, l); }
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
            Message::ApplyScene(i) => self.apply_scene(i),
            Message::SaveDefault => self.run_selected("set_default", |c| async move { c.set_default().await }),
            Message::StopFlow => self.run_selected("stop_cf", |c| async move { c.stop_cf().await }),
            Message::ApplyFlowPreset(i) => self.apply_flow_preset(i),
            Message::FlowRowAdd => {
                if let Some(id) = self.selected_id() {
                    self.flow_rows.entry(id).or_default().push(FlowRow::default());
                }
                Task::none()
            }
            Message::FlowRowDel(row) => {
                if let Some(id) = self.selected_id()
                    && let Some(rows) = self.flow_rows.get_mut(&id)
                    && row < rows.len()
                {
                    rows.remove(row);
                }
                Task::none()
            }
            Message::FlowRowEdit { row, field, value } => {
                if let Some(id) = self.selected_id()
                    && let Some(rows) = self.flow_rows.get_mut(&id)
                    && let Some(r) = rows.get_mut(row)
                {
                    r.set(field, value);
                }
                Task::none()
            }
            Message::FlowCountEdit(s) => {
                if let Some(id) = self.selected_id() { self.flow_count.insert(id, s); }
                Task::none()
            }
            Message::StartCustomFlow => self.start_custom_flow(),
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
                Task::none()
            }
            Message::SelectSettingsTab(tab) => {
                self.settings_tab = tab;
                Task::none()
            }
            Message::ThemeChanged(pref) => {
                self.theme = resolve_theme(&pref);
                self.theme_pref = pref;
                Task::none()
            }
            Message::TimeoutChanged(s) => {
                let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                self.timeout_secs = digits.parse().unwrap_or(0);
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
                if let Some(d) = self.devices.iter_mut().find(|d| d.id == id) {
                    apply_props(&mut d.state, &params);
                }
                Task::none()
            }
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

    /// Which light controls currently target for the selected device.
    pub(crate) fn target_light(&self) -> crate::message::Light {
        self.selected_id()
            .and_then(|id| self.target.get(&id).copied())
            .unwrap_or_default()
    }

    /// The active detail tab for the selected device.
    pub(crate) fn active_tab(&self) -> crate::message::DetailTab {
        self.selected_id()
            .and_then(|id| self.tabs.get(&id).copied())
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
    /// targeted light.
    fn apply_scene(&mut self, i: usize) -> Task<Message> {
        let Some(p) = crate::presets::SCENES.get(i) else { return Task::none() };
        let scene = (p.make)();
        let bg = self.target_light().is_bg();
        self.run_selected("scene applied", move |c| async move {
            if bg { c.bg_set_scene(scene).await } else { c.set_scene(scene).await }
        })
    }
    /// Apply a preset flow (by index into [`crate::presets::FLOWS`]) to the
    /// targeted light.
    fn apply_flow_preset(&mut self, i: usize) -> Task<Message> {
        let Some(p) = crate::presets::FLOWS.get(i) else { return Task::none() };
        let (count, action, expr) = (p.count, p.action, (p.make)());
        let bg = self.target_light().is_bg();
        self.run_selected("flow started", move |c| async move {
            if bg {
                c.bg_start_cf(count, action, expr).await
            } else {
                c.start_cf(count, action, expr).await
            }
        })
    }

    /// Start the custom flow from the editor draft. Validates the expression
    /// locally (non-empty, each step >=50ms) before sending so a bad draft
    /// surfaces in the status bar instead of a device rejection.
    fn start_custom_flow(&mut self) -> Task<Message> {
        let Some(id) = self.selected_id() else { return Task::none() };
        let rows = self.flow_rows.get(&id).cloned().unwrap_or_default();
        let count = self.flow_count.get(&id).and_then(|s| s.parse().ok()).unwrap_or(0u32);
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
        let bg = self.target_light().is_bg();
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
    let stream = BroadcastStream::new(sub.client.notifications()).filter_map(move |res| {
        res.ok().map(|n| Message::StateChanged {
            id: id.clone(),
            params: n.params,
        })
    });
    Box::pin(stream)
}

/// Merge a `props` notification (all string values) into the local [`State`].
fn apply_props(state: &mut State, params: &HashMap<String, String>) {
    for (k, v) in params {
        match k.as_str() {
            "power" => state.power = Some(v == "on"),
            "bright" => state.bright = v.parse().ok().or(state.bright),
            "rgb" => state.rgb = v.parse().ok().or(state.rgb),
            "ct" => state.ct = v.parse().ok().or(state.ct),
            "hue" => state.hue = v.parse().ok().or(state.hue),
            "sat" => state.sat = v.parse().ok().or(state.sat),
            "color_mode" => state.color_mode = v.parse().ok().or(state.color_mode),
            "name" => state.name = Some(v.clone()),
            _ => {}
        }
    }
}

/// Show a native yes/no popup asking to open the (closed) discovery port.
/// Returns `true` if the user agreed. On yes, the privileged `ufw allow` runs via
/// `pkexec`, so the system's own polkit dialog collects the password — no terminal.
async fn ask_open_port() -> bool {
    rfd::AsyncMessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Yeelight Studio")
        .set_description(format!(
            "Discovery port {}/udp is closed in the firewall.\n\nOpen it now? \
             Your system will ask for your password.",
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
    fn color_u32_round_trips() {
        for rgb in [0x000000, 0xFF0000, 0x00FF00, 0x0000FF, 0x123456, 0xFFFFFF] {
            assert_eq!(color_to_u32(u32_to_color(rgb)), rgb);
        }
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
