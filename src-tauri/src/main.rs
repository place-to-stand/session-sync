// Prevents an additional console window on Windows in release mode.
// Not strictly necessary for a macOS-only app, but harmless and conventional.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    session_sync_lib::run();
}
