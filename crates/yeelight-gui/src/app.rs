//! Application state, the update loop, and the async command plumbing.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use iced::{Color, Element, Subscription, Task};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use yeelight_core::{Client, Effect, State, discovery, firewall};

use crate::message::{Btn, CmdKind, Message, Op, OpKey, Sidebar, SettingsTab, ThemePref};
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
    pub(crate) sidebar: Sidebar,
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
}

impl Default for App {
    fn default() -> Self {
        Self {
            sidebar: Sidebar::default(),
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
        }
    }
}

impl App {
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
                self.scanning = true;
                self.scan_progress = 0.0;
                self.status = Status::Ok("scanning…".into());
                let secs = self.timeout_secs.max(1) as u64;
                Task::perform(
                    async move {
                        // Best-effort: no-op if ufw is missing/inactive.
                        let _ = firewall::ensure_udp_open(discovery::SSDP_PORT).await;
                        discovery::search(Duration::from_secs(secs))
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::Scanned,
                )
            }
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
            Message::SelectSidebar(s) => {
                self.sidebar = s;
                Task::none()
            }
            Message::SelectTab(i) => {
                if i < self.devices.len() {
                    self.selected = Some(i);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_u32_round_trips() {
        for rgb in [0x000000, 0xFF0000, 0x00FF00, 0x0000FF, 0x123456, 0xFFFFFF] {
            assert_eq!(color_to_u32(u32_to_color(rgb)), rgb);
        }
    }
}
