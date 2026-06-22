//! Settings pane: a General tab (discovery + control behaviour) and an
//! Appearance tab (theme selection).

use iced::widget::{button, checkbox, column, container, pick_list, row, text, text_input};
use iced::{Element, Length::Fill};

use crate::app::App;
use crate::message::{Message, SettingsTab, ThemePref};

/// Render the settings pane: sub-tab bar over the selected tab's content.
pub(crate) fn pane(app: &App) -> Element<'_, Message> {
    let tab = |label: &str, target: SettingsTab| {
        let marker = if app.settings_tab == target { "● " } else { "" };
        button(text(format!("{marker}{label}"))).on_press(Message::SelectSettingsTab(target))
    };
    let tabs = row![
        tab("General", SettingsTab::General),
        tab("Appearance", SettingsTab::Appearance),
    ]
    .spacing(6);

    let content = match app.settings_tab {
        SettingsTab::General => general(app),
        SettingsTab::Appearance => appearance(app),
    };

    container(column![text("Settings").size(22), tabs, content].spacing(16))
        .padding(20)
        .width(Fill)
        .height(Fill)
        .into()
}

/// General tab: discover timeout, the fixed multicast group, and the force toggle.
fn general(app: &App) -> Element<'_, Message> {
    let timeout = row![
        text("Discover timeout (s):").width(180),
        text_input("3", &app.timeout_secs.to_string())
            .on_input(Message::TimeoutChanged)
            .width(80),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let addr = row![
        text("Discovery group:").width(180),
        text(format!(
            "{}:{} (multicast, fixed)",
            yeelight_core::discovery::SSDP_ADDR,
            yeelight_core::discovery::SSDP_PORT,
        )),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let force = checkbox(app.force_all)
        .label("Enable all controls (ignore device support)")
        .on_toggle(Message::ForceAllToggled);

    column![timeout, addr, force].spacing(16).into()
}

/// Appearance tab: pick any built-in iced theme, or follow the OS.
fn appearance(app: &App) -> Element<'_, Message> {
    let prefs: Vec<ThemePref> = std::iter::once(ThemePref::System)
        .chain(iced::Theme::ALL.iter().cloned().map(ThemePref::Fixed))
        .collect();
    let list = pick_list(prefs, Some(app.theme_pref.clone()), Message::ThemeChanged);
    row![text("Theme:").width(180), list]
        .spacing(10)
        .align_y(iced::Center)
        .into()
}
