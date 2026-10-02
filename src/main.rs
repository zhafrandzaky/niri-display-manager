//! Application entry point.
//!
//! This binary is a thin boundary: logging setup, command-line parsing, error
//! reporting, and the GTK application runner. All behaviour lives in the
//! library layers.

#![forbid(unsafe_code)]

fn main() -> glib::ExitCode {
    init_logging();
    let raw_args: Vec<String> = std::env::args().collect();
    let parsed = match niri_display_manager::cli::parse(&raw_args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("niri-display-manager: {error}");
            eprint!("{}", niri_display_manager::cli::help_text());
            return glib::ExitCode::FAILURE;
        }
    };
    if parsed.options.help {
        print!("{}", niri_display_manager::cli::help_text());
        return glib::ExitCode::SUCCESS;
    }

    match niri_display_manager::presentation::run(parsed) {
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
