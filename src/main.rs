#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod encoder;
mod decoder;
mod viewer;
mod crypto;

fn main() -> eframe::Result {
    // Запускаем GUI просмотрщик
    viewer::run_viewer()
}
