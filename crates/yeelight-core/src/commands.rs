//! Typed control methods (spec §4.1) implemented on [`Client`].
//!
//! Every method maps directly to a spec command. `bg_*` variants mirror their `set_*`
//! counterparts for the background light. For anything not wrapped here use
//! [`Client::call`].

use serde_json::{Value, json};

use crate::client::Client;
use crate::error::{Error, Result};

/// Transition effect for a state change (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Jump directly to the target; duration is ignored.
    Sudden,
    /// Gradual change over the given duration in ms (minimum 30).
    Smooth(u32),
}

impl Effect {
    /// Render to `(effect, duration)`, validating the smooth minimum.
    fn params(self) -> Result<(&'static str, u32)> {
        match self {
            Effect::Sudden => Ok(("sudden", 0)),
            Effect::Smooth(d) if d >= 30 => Ok(("smooth", d)),
            Effect::Smooth(d) => Err(Error::InvalidParam(format!("smooth duration {d}ms < 30ms"))),
        }
    }
}

/// Optional power-on mode (`set_power` param 4, spec §4.1).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum PowerMode {
    /// Normal turn on (default).
    Normal = 0,
    /// Turn on and switch to color-temperature mode.
    Ct = 1,
    /// Turn on and switch to RGB mode.
    Rgb = 2,
    /// Turn on and switch to HSV mode.
    Hsv = 3,
    /// Turn on and switch to color-flow mode.
    ColorFlow = 4,
    /// Turn on and switch to night-light mode (ceiling light only).
    NightLight = 5,
}

/// Action taken when a color flow stops (`start_cf` param 2).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum FlowAction {
    /// Recover to the state before the flow started.
    Recover = 0,
    /// Stay at the state when the flow stopped.
    Stay = 1,
    /// Turn the light off after the flow stopped.
    Off = 2,
}

/// Direction for `set_adjust` (spec §4.1).
#[derive(Debug, Clone, Copy)]
pub enum AdjustAction {
    /// Increase the property.
    Increase,
    /// Decrease the property.
    Decrease,
    /// Increase, wrapping to minimum after maximum.
    Circle,
}

impl AdjustAction {
    fn as_str(self) -> &'static str {
        match self {
            AdjustAction::Increase => "increase",
            AdjustAction::Decrease => "decrease",
            AdjustAction::Circle => "circle",
        }
    }
}

/// Property targeted by `set_adjust` (spec §4.1).
#[derive(Debug, Clone, Copy)]
pub enum AdjustProp {
    /// Brightness.
    Bright,
    /// Color temperature.
    Ct,
    /// Color (only valid with [`AdjustAction::Circle`]).
    Color,
}

impl AdjustProp {
    fn as_str(self) -> &'static str {
        match self {
            AdjustProp::Bright => "bright",
            AdjustProp::Ct => "ct",
            AdjustProp::Color => "color",
        }
    }
}

/// Cron job type (`cron_*`); currently only power-off (spec §4.1).
#[derive(Debug, Clone, Copy)]
#[repr(i64)]
pub enum CronType {
    /// Power off after the timer.
    PowerOff = 0,
}

/// One step of a color flow (spec `start_cf` flow tuple).
#[derive(Debug, Clone, Copy)]
pub struct FlowTuple {
    /// Gradual change / sleep time in ms (minimum 50).
    pub duration: u32,
    /// `1` color, `2` color temperature, `7` sleep.
    pub mode: u8,
    /// RGB value (mode 1) or CT value (mode 2); ignored for sleep.
    pub value: u32,
    /// `-1` keeps current brightness, or `1..=100`; ignored for sleep.
    pub brightness: i8,
}

/// A color-flow expression: an ordered series of [`FlowTuple`]s.
#[derive(Debug, Clone, Default)]
pub struct FlowExpr(pub Vec<FlowTuple>);

impl FlowExpr {
    /// Render to the comma-separated string the device expects.
    pub fn render(&self) -> Result<String> {
        if self.0.is_empty() {
            return Err(Error::InvalidParam("empty flow expression".to_string()));
        }
        let mut parts = Vec::with_capacity(self.0.len() * 4);
        for t in &self.0 {
            if t.duration < 50 {
                return Err(Error::InvalidParam(format!(
                    "flow duration {}ms < 50ms",
                    t.duration
                )));
            }
            parts.push(t.duration.to_string());
            parts.push(t.mode.to_string());
            parts.push(t.value.to_string());
            parts.push(t.brightness.to_string());
        }
        Ok(parts.join(","))
    }

    /// Number of tuples in the expression.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the expression has no tuples.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A scene for `set_scene` (spec §4.1). Accepted whether the light is on or off.
#[derive(Debug, Clone)]
pub enum Scene {
    /// Set color + brightness.
    Color {
        /// RGB value.
        rgb: u32,
        /// Brightness `1..=100`.
        bright: u8,
    },
    /// Set hue/sat + brightness.
    Hsv {
        /// Hue `0..=359`.
        hue: u16,
        /// Saturation `0..=100`.
        sat: u8,
        /// Brightness `1..=100`.
        bright: u8,
    },
    /// Set color temperature + brightness.
    Ct {
        /// Color temperature `1700..=6500` K.
        ct: u16,
        /// Brightness `1..=100`.
        bright: u8,
    },
    /// Start a color flow.
    ColorFlow {
        /// Number of visible changes before stopping; `0` = infinite.
        count: u32,
        /// Action after the flow stops.
        action: FlowAction,
        /// The flow expression.
        expr: FlowExpr,
    },
    /// Turn on to a brightness, then auto-off after `minutes`.
    AutoDelayOff {
        /// Brightness `1..=100`.
        bright: u8,
        /// Sleep timer in minutes.
        minutes: u32,
    },
}

impl Scene {
    fn params(self) -> Result<Vec<Value>> {
        Ok(match self {
            Scene::Color { rgb, bright } => vec![json!("color"), json!(rgb), json!(bright)],
            Scene::Hsv { hue, sat, bright } => {
                vec![json!("hsv"), json!(hue), json!(sat), json!(bright)]
            }
            Scene::Ct { ct, bright } => vec![json!("ct"), json!(ct), json!(bright)],
            Scene::ColorFlow {
                count,
                action,
                expr,
            } => vec![
                json!("cf"),
                json!(count),
                json!(action as u8),
                json!(expr.render()?),
            ],
            Scene::AutoDelayOff { bright, minutes } => {
                vec![json!("auto_delay_off"), json!(bright), json!(minutes)]
            }
        })
    }
}

impl Client {
    /// Call a supported method and discard the `["ok"]` result.
    async fn ok(&self, method: &str, params: Vec<Value>) -> Result<()> {
        self.call_supported(method, params).await.map(|_| ())
    }

    // ----- queries -------------------------------------------------------------

    /// `get_prop` — read the given property names; unknown names return `""` (spec §4.1).
    pub async fn get_prop(&self, props: &[&str]) -> Result<Vec<String>> {
        let params = props.iter().map(|p| json!(p)).collect();
        let v = self.call_supported("get_prop", params).await?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default())
    }

    // ----- color / temperature / brightness ------------------------------------

    /// `set_ct_abx` — set color temperature `1700..=6500` K. Only when the light is on.
    pub async fn set_ct_abx(&self, ct: u16, effect: Effect) -> Result<()> {
        self.set_ct_m("set_ct_abx", ct, effect).await
    }

    /// `set_rgb` — set color (`0..=0xFFFFFF`). Only when the light is on.
    pub async fn set_rgb(&self, rgb: u32, effect: Effect) -> Result<()> {
        self.set_rgb_m("set_rgb", rgb, effect).await
    }

    /// `set_hsv` — set hue (`0..=359`) and saturation (`0..=100`). Only when on.
    pub async fn set_hsv(&self, hue: u16, sat: u8, effect: Effect) -> Result<()> {
        self.set_hsv_m("set_hsv", hue, sat, effect).await
    }

    /// `set_bright` — set brightness `1..=100`. Only when the light is on.
    pub async fn set_bright(&self, bright: u8, effect: Effect) -> Result<()> {
        self.set_bright_m("set_bright", bright, effect).await
    }

    /// `set_power` — turn on/off, optionally switching to a [`PowerMode`].
    pub async fn set_power(&self, on: bool, effect: Effect, mode: Option<PowerMode>) -> Result<()> {
        self.set_power_m("set_power", on, effect, mode).await
    }

    /// `toggle` — flip the main light on/off.
    pub async fn toggle(&self) -> Result<()> {
        self.ok("toggle", vec![]).await
    }

    /// `set_default` — save the current state as power-on default. Only when on.
    pub async fn set_default(&self) -> Result<()> {
        self.ok("set_default", vec![]).await
    }

    // ----- color flow ----------------------------------------------------------

    /// `start_cf` — start a color flow. `count` is visible changes before stop (`0` = infinite).
    pub async fn start_cf(&self, count: u32, action: FlowAction, expr: FlowExpr) -> Result<()> {
        self.start_cf_m("start_cf", count, action, expr).await
    }

    /// `stop_cf` — stop a running color flow.
    pub async fn stop_cf(&self) -> Result<()> {
        self.ok("stop_cf", vec![]).await
    }

    /// `set_scene` — set the light directly to a [`Scene`].
    pub async fn set_scene(&self, scene: Scene) -> Result<()> {
        self.set_scene_m("set_scene", scene).await
    }

    // ----- cron ----------------------------------------------------------------

    /// `cron_add` — start a timer job of `minutes`. Only when the light is on.
    pub async fn cron_add(&self, ty: CronType, minutes: u32) -> Result<()> {
        self.ok("cron_add", vec![json!(ty as i64), json!(minutes)]).await
    }

    /// `cron_get` — retrieve the current cron job (raw value).
    pub async fn cron_get(&self, ty: CronType) -> Result<Value> {
        self.call_supported("cron_get", vec![json!(ty as i64)]).await
    }

    /// `cron_del` — stop the specified cron job.
    pub async fn cron_del(&self, ty: CronType) -> Result<()> {
        self.ok("cron_del", vec![json!(ty as i64)]).await
    }

    // ----- adjust / misc -------------------------------------------------------

    /// `set_adjust` — relative change without knowing the current value.
    ///
    /// [`AdjustProp::Color`] requires [`AdjustAction::Circle`] (spec §4.1).
    pub async fn set_adjust(&self, action: AdjustAction, prop: AdjustProp) -> Result<()> {
        self.set_adjust_m("set_adjust", action, prop).await
    }

    /// `set_name` — name the device (stored on the device, max 64 bytes).
    pub async fn set_name(&self, name: &str) -> Result<()> {
        self.ok("set_name", vec![json!(name)]).await
    }

    /// `adjust_bright` — change brightness by `percentage` (`-100..=100`) over `duration` ms.
    pub async fn adjust_bright(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("adjust_bright", percentage, duration).await
    }

    /// `adjust_ct` — change color temperature by `percentage` over `duration` ms.
    pub async fn adjust_ct(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("adjust_ct", percentage, duration).await
    }

    /// `adjust_color` — cycle color over `duration` ms (percentage is ignored by the device).
    pub async fn adjust_color(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("adjust_color", percentage, duration).await
    }

    /// `dev_toggle` — toggle the main light and the background light together.
    pub async fn dev_toggle(&self) -> Result<()> {
        self.ok("dev_toggle", vec![]).await
    }

    // ----- music mode (low level; see [`crate::music`]) ------------------------

    /// `set_music` enable — tell the device to connect back to `host:port`.
    ///
    /// Prefer [`crate::music::MusicConnection::start`], which also opens the firewall
    /// and accepts the connect-back.
    pub async fn set_music_on(&self, host: std::net::IpAddr, port: u16) -> Result<()> {
        self.ok("set_music", vec![json!(1), json!(host.to_string()), json!(port)])
            .await
    }

    /// `set_music` disable.
    pub async fn set_music_off(&self) -> Result<()> {
        self.ok("set_music", vec![json!(0)]).await
    }

    // ----- background light (`bg_*`) -------------------------------------------

    /// `bg_set_ct_abx` — background-light color temperature.
    pub async fn bg_set_ct_abx(&self, ct: u16, effect: Effect) -> Result<()> {
        self.set_ct_m("bg_set_ct_abx", ct, effect).await
    }

    /// `bg_set_rgb` — background-light color.
    pub async fn bg_set_rgb(&self, rgb: u32, effect: Effect) -> Result<()> {
        self.set_rgb_m("bg_set_rgb", rgb, effect).await
    }

    /// `bg_set_hsv` — background-light hue/saturation.
    pub async fn bg_set_hsv(&self, hue: u16, sat: u8, effect: Effect) -> Result<()> {
        self.set_hsv_m("bg_set_hsv", hue, sat, effect).await
    }

    /// `bg_set_bright` — background-light brightness.
    pub async fn bg_set_bright(&self, bright: u8, effect: Effect) -> Result<()> {
        self.set_bright_m("bg_set_bright", bright, effect).await
    }

    /// `bg_set_power` — background-light power.
    pub async fn bg_set_power(&self, on: bool, effect: Effect, mode: Option<PowerMode>) -> Result<()> {
        self.set_power_m("bg_set_power", on, effect, mode).await
    }

    /// `bg_toggle` — toggle only the background light.
    pub async fn bg_toggle(&self) -> Result<()> {
        self.ok("bg_toggle", vec![]).await
    }

    /// `bg_set_default` — save background-light state as default.
    pub async fn bg_set_default(&self) -> Result<()> {
        self.ok("bg_set_default", vec![]).await
    }

    /// `bg_start_cf` — start a background-light color flow.
    pub async fn bg_start_cf(&self, count: u32, action: FlowAction, expr: FlowExpr) -> Result<()> {
        self.start_cf_m("bg_start_cf", count, action, expr).await
    }

    /// `bg_stop_cf` — stop a background-light color flow.
    pub async fn bg_stop_cf(&self) -> Result<()> {
        self.ok("bg_stop_cf", vec![]).await
    }

    /// `bg_set_scene` — set the background light to a [`Scene`].
    pub async fn bg_set_scene(&self, scene: Scene) -> Result<()> {
        self.set_scene_m("bg_set_scene", scene).await
    }

    /// `bg_set_adjust` — relative change of the background light.
    pub async fn bg_set_adjust(&self, action: AdjustAction, prop: AdjustProp) -> Result<()> {
        self.set_adjust_m("bg_set_adjust", action, prop).await
    }

    /// `bg_adjust_bright` — change background brightness by `percentage` over `duration` ms.
    pub async fn bg_adjust_bright(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("bg_adjust_bright", percentage, duration).await
    }

    /// `bg_adjust_ct` — change background color temperature by `percentage` over `duration` ms.
    pub async fn bg_adjust_ct(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("bg_adjust_ct", percentage, duration).await
    }

    /// `bg_adjust_color` — cycle background color over `duration` ms.
    pub async fn bg_adjust_color(&self, percentage: i8, duration: u32) -> Result<()> {
        self.adjust_m("bg_adjust_color", percentage, duration).await
    }

    // ----- shared builders (main + bg share param shapes) ----------------------

    async fn set_ct_m(&self, method: &str, ct: u16, effect: Effect) -> Result<()> {
        let (e, d) = effect.params()?;
        self.ok(method, vec![json!(ct), json!(e), json!(d)]).await
    }

    async fn set_rgb_m(&self, method: &str, rgb: u32, effect: Effect) -> Result<()> {
        let (e, d) = effect.params()?;
        self.ok(method, vec![json!(rgb), json!(e), json!(d)]).await
    }

    async fn set_hsv_m(&self, method: &str, hue: u16, sat: u8, effect: Effect) -> Result<()> {
        let (e, d) = effect.params()?;
        self.ok(method, vec![json!(hue), json!(sat), json!(e), json!(d)]).await
    }

    async fn set_bright_m(&self, method: &str, bright: u8, effect: Effect) -> Result<()> {
        let (e, d) = effect.params()?;
        self.ok(method, vec![json!(bright), json!(e), json!(d)]).await
    }

    async fn set_power_m(
        &self,
        method: &str,
        on: bool,
        effect: Effect,
        mode: Option<PowerMode>,
    ) -> Result<()> {
        let (e, d) = effect.params()?;
        let mut p = vec![json!(if on { "on" } else { "off" }), json!(e), json!(d)];
        if let Some(m) = mode {
            p.push(json!(m as u8));
        }
        self.ok(method, p).await
    }

    async fn start_cf_m(
        &self,
        method: &str,
        count: u32,
        action: FlowAction,
        expr: FlowExpr,
    ) -> Result<()> {
        let expr = expr.render()?;
        self.ok(method, vec![json!(count), json!(action as u8), json!(expr)]).await
    }

    async fn set_scene_m(&self, method: &str, scene: Scene) -> Result<()> {
        self.ok(method, scene.params()?).await
    }

    async fn set_adjust_m(&self, method: &str, action: AdjustAction, prop: AdjustProp) -> Result<()> {
        if matches!(prop, AdjustProp::Color) && !matches!(action, AdjustAction::Circle) {
            return Err(Error::InvalidParam(
                "adjust color supports only the 'circle' action".to_string(),
            ));
        }
        self.ok(method, vec![json!(action.as_str()), json!(prop.as_str())]).await
    }

    async fn adjust_m(&self, method: &str, percentage: i8, duration: u32) -> Result<()> {
        self.ok(method, vec![json!(percentage), json!(duration)]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_expr_renders_spec_example() {
        let expr = FlowExpr(vec![
            FlowTuple { duration: 1000, mode: 2, value: 2700, brightness: 100 },
            FlowTuple { duration: 500, mode: 1, value: 255, brightness: 10 },
            FlowTuple { duration: 5000, mode: 7, value: 0, brightness: 0 },
            FlowTuple { duration: 500, mode: 2, value: 5000, brightness: 1 },
        ]);
        assert_eq!(
            expr.render().unwrap(),
            "1000,2,2700,100,500,1,255,10,5000,7,0,0,500,2,5000,1"
        );
    }

    #[test]
    fn empty_flow_expr_is_rejected() {
        assert!(matches!(FlowExpr::default().render(), Err(Error::InvalidParam(_))));
    }

    #[test]
    fn smooth_rejects_short_duration() {
        assert!(matches!(Effect::Smooth(10).params(), Err(Error::InvalidParam(_))));
    }

    #[test]
    fn smooth_accepts_minimum_duration() {
        assert_eq!(Effect::Smooth(30).params().unwrap(), ("smooth", 30));
    }

    #[test]
    fn sudden_has_zero_duration() {
        assert_eq!(Effect::Sudden.params().unwrap(), ("sudden", 0));
    }

    #[test]
    fn ct_scene_params() {
        let p = Scene::Ct { ct: 5400, bright: 100 }.params().unwrap();
        assert_eq!(p, vec![json!("ct"), json!(5400), json!(100)]);
    }
}
