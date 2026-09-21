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
    // Panic trace into the diagnostics ring (the diagnostics export's tail
    // source): release aborts on panic, so the default report never reaches
    // any sink. platform::log needs no initialization, so the hook is safe
    // from the first statement on; the abort behavior itself is unchanged.
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write as _;
        let loc = info.location().map_or_else(
            || "unknown".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        let line = format!(
            "panic 于 {loc}: {}",
            info.payload_as_str().unwrap_or("<非字符串 payload>")
        );
        platform::log(&line);
        // Keep the console report the default hook used to print (debug
        // runs); the release GUI subsystem has no stderr — swallow that.
        let _ = writeln!(std::io::stderr(), "{}{line}", platform::LOG_PREFIX);
    }));
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
