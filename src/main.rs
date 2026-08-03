mod cli;
#[cfg(feature = "gui")]
mod gui;

#[cfg(feature = "gui")]
pub mod icon_finder;
pub mod models;
#[cfg(all(feature = "gui", target_os = "linux"))]
pub mod package_manager;
pub mod search;

fn main() {
    let Some(launch_options) = cli::handle_arguments() else {
        return;
    };

    #[cfg(feature = "gui")]
    if let Err(error) = gui::run(launch_options.system_font) {
        eprintln!("Failed to start the GUI: {error}");
        std::process::exit(1);
    }

    #[cfg(not(feature = "gui"))]
    {
        let _ = launch_options;
    }
}
