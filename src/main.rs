//! Quotify — a tray widget for GLM Coding Plan usage on the Windows taskbar
//!
//! Layering: `app` assembly / `api` data / `ui` interface / `platform` platform services
//! / `service` background services

// Release builds use the GUI subsystem: closing the console window would kill the
// process. Debug keeps a console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod locks;
mod platform;
mod service;
mod ui;

fn main() {
    // The api layer's log sink is injected here: the data layer stays platform-agnostic.
    api::set_log_hook(platform::log);
    // Single instance: when an instance already exists, wake it to pop out the panel,
    // then exit silently.
    let guard = platform::instance::acquire();
    if !guard.is_first() {
        guard.wake_existing();
        return;
    }
    std::process::exit(app::run());
}
