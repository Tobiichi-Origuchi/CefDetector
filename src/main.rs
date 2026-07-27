#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod gui;

pub mod icon_finder;
pub mod models;
pub mod package_manager;
pub mod search;

fn main() {
    cli::handle_cli();

    if let Err(error) = gui::run() {
        eprintln!("Failed to start the GUI: {error}");
        std::process::exit(1);
    }
}
