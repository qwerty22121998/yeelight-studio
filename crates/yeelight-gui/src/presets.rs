//! Static preset tables surfaced by the control tabs: white temperatures,
//! scenes, and color-flow presets. Kept here (not in the view) so the
//! preset → core-type construction is pure and unit-testable.

use yeelight_core::{FlowAction, FlowExpr, FlowTuple, Scene};

/// Named color-temperature presets (Kelvin), all within the device's 1700–6500 range.
pub(crate) const TEMPS: &[(&str, u16)] = &[
    ("Candle", 1700),
    ("Warm", 2700),
    ("Neutral", 4000),
    ("Daylight", 5000),
    ("Cool", 6500),
];

/// A named scene preset. `make` builds a fresh [`Scene`] (cf scenes own a `FlowExpr`).
pub(crate) struct ScenePreset {
    /// Display name.
    pub(crate) name: &'static str,
    /// Builds the scene to apply.
    pub(crate) make: fn() -> Scene,
}

/// Curated scenes shown in the Scenes tab.
pub(crate) const SCENES: &[ScenePreset] = &[
    ScenePreset { name: "Reading", make: || Scene::Ct { ct: 4000, bright: 100 } },
    ScenePreset { name: "Relax",   make: || Scene::Ct { ct: 2700, bright: 40 } },
    ScenePreset { name: "Sunset",  make: || Scene::Color { rgb: 0xFF5E3A, bright: 60 } },
    ScenePreset { name: "Movie",   make: || Scene::Color { rgb: 0x1A237E, bright: 20 } },
    ScenePreset { name: "Party",   make: || Scene::ColorFlow {
        count: 0,
        action: FlowAction::Recover,
        expr: party_flow(),
    } },
];

/// A named color-flow preset for the Flow tab.
pub(crate) struct FlowPreset {
    /// Display name.
    pub(crate) name: &'static str,
    /// Visible changes before stopping (`0` = infinite).
    pub(crate) count: u32,
    /// Action after the flow stops.
    pub(crate) action: FlowAction,
    /// Builds the flow expression.
    pub(crate) make: fn() -> FlowExpr,
}

/// Curated flows shown in the Flow tab.
pub(crate) const FLOWS: &[FlowPreset] = &[
    FlowPreset { name: "Pulse",  count: 0, action: FlowAction::Recover, make: || FlowExpr(vec![
        FlowTuple { duration: 1000, mode: 1, value: 0xFF0000, brightness: 100 },
        FlowTuple { duration: 1000, mode: 1, value: 0xFF0000, brightness: 1 },
    ]) },
    FlowPreset { name: "Police", count: 0, action: FlowAction::Recover, make: || FlowExpr(vec![
        FlowTuple { duration: 300, mode: 1, value: 0x0000FF, brightness: 100 },
        FlowTuple { duration: 300, mode: 1, value: 0xFF0000, brightness: 100 },
    ]) },
    FlowPreset { name: "Candle", count: 0, action: FlowAction::Recover, make: || FlowExpr(vec![
        FlowTuple { duration: 800, mode: 2, value: 2000, brightness: 60 },
        FlowTuple { duration: 800, mode: 2, value: 2700, brightness: 90 },
        FlowTuple { duration: 1200, mode: 2, value: 2200, brightness: 40 },
    ]) },
    FlowPreset { name: "Sunrise", count: 1, action: FlowAction::Stay, make: || FlowExpr(vec![
        FlowTuple { duration: 50, mode: 1, value: 0x331400, brightness: 1 },
        FlowTuple { duration: 360_000, mode: 2, value: 1700, brightness: 10 },
        FlowTuple { duration: 540_000, mode: 2, value: 2700, brightness: 100 },
    ]) },
];

fn party_flow() -> FlowExpr {
    FlowExpr(vec![
        FlowTuple { duration: 500, mode: 1, value: 0xFF0000, brightness: 100 },
        FlowTuple { duration: 500, mode: 1, value: 0x00FF00, brightness: 100 },
        FlowTuple { duration: 500, mode: 1, value: 0x0000FF, brightness: 100 },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temps_are_in_device_range() {
        assert!(!TEMPS.is_empty());
        for (name, k) in TEMPS {
            assert!((1700..=6500).contains(k), "{name} = {k}K out of range");
        }
    }

    #[test]
    fn flow_presets_render_valid_expressions() {
        // Every flow tuple must be >= 50ms or FlowExpr::render rejects it.
        for p in FLOWS {
            assert!((p.make)().render().is_ok(), "flow preset {} renders invalid", p.name);
        }
    }

    #[test]
    fn cf_scene_flow_renders() {
        // The only scene whose validity we can check publicly is the cf one.
        let Scene::ColorFlow { expr, .. } = (SCENES.iter().find(|s| s.name == "Party").unwrap().make)()
        else { panic!("Party scene should be a color flow") };
        assert!(expr.render().is_ok());
    }
}
