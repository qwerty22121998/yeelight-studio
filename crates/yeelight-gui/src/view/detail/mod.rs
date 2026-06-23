//! Detail pane: header + hero + tabbed controls for the selected device.

use iced::widget::{container, text};
use iced::{Element, Length::Fill};

use crate::app::App;
use crate::message::Message;
use yeelight_core::Device;

/// Render the detail pane for the selected device.
pub(crate) fn pane(app: &App) -> Element<'_, Message> {
    if app.devices.is_empty() {
        return container(text("No devices. Press Scan to discover bulbs on the LAN."))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into();
    }
    let Some(d) = app.selected.and_then(|i| app.devices.get(i)) else {
        return container(text("Select a device."))
            .padding(20)
            .width(Fill)
            .height(Fill)
            .into();
    };
    container(text(label_for(d)).size(22))
        .padding(16)
        .width(Fill)
        .height(Fill)
        .into()
}

/// A short label: device name if set, else model + short id.
pub(crate) fn label_for(d: &Device) -> String {
    if let Some(name) = &d.state.name
        && !name.is_empty()
    {
        return name.clone();
    }
    let model = String::from(d.model.clone());
    let short = d.id.rsplit(':').next().unwrap_or(&d.id);
    let short = &short[short.len().saturating_sub(6)..];
    format!("{model} {short}")
}
