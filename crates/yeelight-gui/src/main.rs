//! Desktop GUI for discovering and controlling Yeelight WiFi LEDs.
//!
//! Thin iced front-end over the `yeelight-core` library: scan the LAN, pick a
//! device from the tab bar, and toggle / recolor its main and background lights.

mod ambient;
mod app;
mod message;
mod presets;
mod settings;
mod theme;
mod view;

use app::App;

fn main() -> iced::Result {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("yeelight_gui=info,yeelight_core=debug")),
        )
        .init();

    iced::application(App::boot, App::update, App::view)
        .title("Yeelight Studio")
        .theme(App::theme)
        .subscription(App::subscription)
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .run()
}
