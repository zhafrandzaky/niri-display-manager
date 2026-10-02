//! Command-line parsing for path overrides and diagnostics.
//!
//! Only options owned by this manager are consumed; everything else is
//! forwarded to GTK so standard toolkit flags keep working. Environment
//! variables are resolved separately in `service::config_paths`.

use std::path::PathBuf;

/// Options consumed by this manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliOptions {
    /// Explicit main niri configuration path (`--config`).
    pub config: Option<PathBuf>,
    /// Explicit managed display file (`--display-config`).
    pub display_config: Option<PathBuf>,
    /// Print resolved paths and exit (`--print-paths`).
    pub print_paths: bool,
    /// Print help and exit (`--help`).
    pub help: bool,
}

/// Result of parsing: consumed options plus arguments forwarded to GTK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub options: CliOptions,
    /// Arguments not consumed here, including `argv[0]`.
    pub forwarded: Vec<String>,
}

/// Errors raised while parsing the command line.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CliError {
    #[error("option {0} requires a value")]
    MissingValue(String),
}

/// Parse command-line arguments.
///
/// Recognized options are removed from the forwarded list; everything else,
/// including toolkit flags such as `--display=`, is passed through untouched.
pub fn parse(args: &[String]) -> Result<ParsedArgs, CliError> {
    let mut options = CliOptions::default();
    let mut forwarded = Vec::new();
    let mut iterator = args.iter();

    if let Some(program) = iterator.next() {
        forwarded.push(program.clone());
    }

    let mut passthrough = false;
    while let Some(argument) = iterator.next() {
        if passthrough {
            forwarded.push(argument.clone());
            continue;
        }
        match argument.as_str() {
            "--" => passthrough = true,
            "--help" | "-h" => options.help = true,
            "--print-paths" => options.print_paths = true,
            "--config" => {
                options.config = Some(PathBuf::from(next_value(&mut iterator, "--config")?));
            }
            "--display-config" => {
                options.display_config = Some(PathBuf::from(next_value(
                    &mut iterator,
                    "--display-config",
                )?));
            }
            other if other.starts_with("--config=") => {
                options.config = Some(PathBuf::from(&other["--config=".len()..]));
            }
            other if other.starts_with("--display-config=") => {
                options.display_config = Some(PathBuf::from(&other["--display-config=".len()..]));
            }
            other => forwarded.push(other.to_owned()),
        }
    }

    Ok(ParsedArgs { options, forwarded })
}

fn next_value<'a>(
    iterator: &mut std::slice::Iter<'a, String>,
    option: &str,
) -> Result<&'a str, CliError> {
    iterator
        .next()
        .map(String::as_str)
        .ok_or_else(|| CliError::MissingValue(option.to_owned()))
}

/// Full help text, including configuration discovery and environment docs.
pub fn help_text() -> String {
    String::from(
        "niri-display-manager - display and mirroring manager for the niri compositor\n\
         \n\
         Usage:\n\
         \x20 niri-display-manager [OPTIONS]\n\
         \n\
         Options:\n\
         \x20 --config <PATH>          Main niri configuration file\n\
         \x20 --display-config <PATH>  Managed display file (forces modular mode)\n\
         \x20 --print-paths            Print resolved paths and mode, then exit\n\
         \x20 -h, --help               Print this help and exit\n\
         \n\
         Configuration discovery for the main config (highest precedence first):\n\
         \x20 1. --config <PATH>\n\
         \x20 2. $NIRI_CONFIG\n\
         \x20 3. $NDM_CONFIG_DIR/niri/config.kdl\n\
         \x20 4. $XDG_CONFIG_HOME/niri/config.kdl\n\
         \x20 5. $HOME/.config/niri/config.kdl\n\
         \n\
         The display target defaults to <main config directory>/cfg/display.kdl.\n\
         When that file is included by the main configuration, the manager keeps\n\
         it in sync (modular mode). Otherwise it manages a fenced section inside\n\
         the main configuration itself (inline portable mode).\n\
         \n\
         Environment:\n\
         \x20 NIRI_CONFIG      Main niri configuration path (niri's own variable).\n\
         \x20 NDM_CONFIG_DIR   Configuration base directory override; intended for\n\
         \x20                  tests and advanced setups.\n\
         \x20 RUST_LOG         Log filter, for example RUST_LOG=debug.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<ParsedArgs, CliError> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
        parse(&owned)
    }

    #[test]
    fn parses_path_overrides_in_both_forms() {
        let parsed = parse_args(&[
            "niri-display-manager",
            "--config",
            "/tmp/a.kdl",
            "--display-config=/tmp/b.kdl",
        ])
        .unwrap();
        assert_eq!(parsed.options.config, Some(PathBuf::from("/tmp/a.kdl")));
        assert_eq!(
            parsed.options.display_config,
            Some(PathBuf::from("/tmp/b.kdl"))
        );
        assert_eq!(parsed.forwarded, vec!["niri-display-manager"]);
    }

    #[test]
    fn forwards_unknown_and_toolkit_arguments() {
        let parsed = parse_args(&[
            "niri-display-manager",
            "--config",
            "/tmp/a.kdl",
            "--display=wayland-1",
            "--gapplication-service",
        ])
        .unwrap();
        assert_eq!(parsed.options.config, Some(PathBuf::from("/tmp/a.kdl")));
        assert_eq!(
            parsed.forwarded,
            vec![
                "niri-display-manager",
                "--display=wayland-1",
                "--gapplication-service"
            ]
        );
    }

    #[test]
    fn double_dash_stops_option_parsing() {
        let parsed = parse_args(&["niri-display-manager", "--", "--config", "/tmp/a.kdl"]).unwrap();
        assert_eq!(parsed.options.config, None);
        assert_eq!(
            parsed.forwarded,
            vec!["niri-display-manager", "--config", "/tmp/a.kdl"]
        );
    }

    #[test]
    fn missing_values_are_reported() {
        let error = parse_args(&["niri-display-manager", "--config"]).unwrap_err();
        assert_eq!(error, CliError::MissingValue("--config".to_owned()));
        let error = parse_args(&["niri-display-manager", "--display-config"]).unwrap_err();
        assert_eq!(error, CliError::MissingValue("--display-config".to_owned()));
    }

    #[test]
    fn help_and_print_paths_flags() {
        let parsed = parse_args(&["niri-display-manager", "--help", "--print-paths"]).unwrap();
        assert!(parsed.options.help);
        assert!(parsed.options.print_paths);
        assert!(help_text().contains("NDM_CONFIG_DIR"));
        assert!(help_text().contains("NIRI_CONFIG"));
        assert!(help_text().is_ascii());
    }
}
