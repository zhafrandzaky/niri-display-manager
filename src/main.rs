//! Application entry point.
//!
//! This binary is a thin boundary: logging setup, error reporting, and the GTK
//! application runner. All behaviour lives in the library layers.

#![forbid(unsafe_code)]

fn main() -> glib::ExitCode {
    init_logging();
    match niri_display_manager::presentation::run() {
        Ok(exit_code) => exit_code,
        Err(error) => {
            log::error!("fatal: {error:#}");
            eprintln!("niri-display-manager: {error:#}");
            glib::ExitCode::FAILURE
        }
    }
}

fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
}
