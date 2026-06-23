//! Desktop GUI for discovering and controlling Yeelight WiFi LEDs.
//!
//! Thin iced front-end over the `yeelight-core` library: scan the LAN, pick a
//! device from the tab bar, and toggle / recolor its main and background lights.

mod app;
mod message;
mod view;

use app::App;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Yeelight Studio")
        .theme(App::theme)
        .subscription(App::subscription)
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .run()
}
