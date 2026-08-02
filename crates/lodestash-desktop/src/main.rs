// Windows: no console window on a release GUI build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    lodestash_desktop::run();
}
