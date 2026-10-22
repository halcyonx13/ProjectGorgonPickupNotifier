#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    let settings = std::env::args().any(|a| a == "--settings");
    if settings {
        win::settings::run();
    } else {
        win::app::run();
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("PG Pickup Notifier runs on Windows. See examples/preview.rs for an offline renderer.");
}
