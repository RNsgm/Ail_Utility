#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    ArchiveUtils::log::init();
    ArchiveUtils::viewer::run_viewer()
}
