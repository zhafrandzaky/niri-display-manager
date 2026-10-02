//! Configuration path discovery.
//!
//! Resolution is separated from environment access so tests can supply a
//! synthetic [`Environment`] without mutating process state.
//!
//! Main config precedence:
//!
//! 1. `--config <PATH>`
//! 2. `$NIRI_CONFIG`
//! 3. `$NDM_CONFIG_DIR/niri/config.kdl`
//! 4. `$XDG_CONFIG_HOME/niri/config.kdl`
//! 5. `$HOME/.config/niri/config.kdl`
//!
//! The display target defaults to `<main config directory>/cfg/display.kdl`
//! and can be overridden with `--display-config <PATH>`.

use std::path::{Path, PathBuf};

use super::backup_service::ConfigError;
use crate::cli::CliOptions;

/// Environment inputs used for path resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    pub niri_config: Option<PathBuf>,
    pub ndm_config_dir: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl Environment {
    /// Read the environment of the current process.
    pub fn from_process() -> Self {
        Self {
            niri_config: std::env::var_os("NIRI_CONFIG").map(PathBuf::from),
            ndm_config_dir: std::env::var_os("NDM_CONFIG_DIR").map(PathBuf::from),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

/// Resolved configuration targets before mode detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPaths {
    pub main_config: PathBuf,
    pub display_candidate: PathBuf,
    /// Whether the display target came from `--display-config`.
    pub display_explicit: bool,
}

/// Resolve configuration paths from CLI options and environment inputs.
pub fn resolve_paths(
    cli: &CliOptions,
    environment: &Environment,
) -> Result<ResolvedPaths, ConfigError> {
    let main_config = if let Some(path) = &cli.config {
        make_absolute(path)?
    } else if let Some(path) = &environment.niri_config {
        make_absolute(path)?
    } else if let Some(base) = &environment.ndm_config_dir {
        make_absolute(&base.join("niri").join("config.kdl"))?
    } else if let Some(base) = &environment.xdg_config_home {
        make_absolute(&base.join("niri").join("config.kdl"))?
    } else if let Some(home) = &environment.home {
        make_absolute(&home.join(".config").join("niri").join("config.kdl"))?
    } else {
        return Err(ConfigError::NoConfigBase);
    };

    let (display_candidate, display_explicit) = match &cli.display_config {
        Some(path) => (make_absolute(path)?, true),
        None => {
            let directory = main_config
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"));
            (directory.join("cfg").join("display.kdl"), false)
        }
    };

    Ok(ResolvedPaths {
        main_config,
        display_candidate,
        display_explicit,
    })
}

fn make_absolute(path: &Path) -> Result<PathBuf, ConfigError> {
    std::path::absolute(path).map_err(|source| ConfigError::InvalidPath {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> Environment {
        Environment {
            niri_config: Some(PathBuf::from("/niri.conf.d/custom.kdl")),
            ndm_config_dir: Some(PathBuf::from("/ndm/config")),
            xdg_config_home: Some(PathBuf::from("/xdg/config")),
            home: Some(PathBuf::from("/home/user")),
        }
    }

    #[test]
    fn cli_config_wins_over_every_environment_source() {
        let cli = CliOptions {
            config: Some(PathBuf::from("/cli/config.kdl")),
            ..CliOptions::default()
        };
        let resolved = resolve_paths(&cli, &environment()).unwrap();
        assert_eq!(resolved.main_config, PathBuf::from("/cli/config.kdl"));
        assert_eq!(
            resolved.display_candidate,
            PathBuf::from("/cli/cfg/display.kdl")
        );
        assert!(!resolved.display_explicit);
    }

    #[test]
    fn environment_precedence_matches_documentation() {
        let cli = CliOptions::default();
        let resolved = resolve_paths(&cli, &environment()).unwrap();
        assert_eq!(
            resolved.main_config,
            PathBuf::from("/niri.conf.d/custom.kdl")
        );

        let mut env = environment();
        env.niri_config = None;
        let resolved = resolve_paths(&cli, &env).unwrap();
        assert_eq!(
            resolved.main_config,
            PathBuf::from("/ndm/config/niri/config.kdl")
        );

        let mut env = environment();
        env.niri_config = None;
        env.ndm_config_dir = None;
        let resolved = resolve_paths(&cli, &env).unwrap();
        assert_eq!(
            resolved.main_config,
            PathBuf::from("/xdg/config/niri/config.kdl")
        );

        let mut env = environment();
        env.niri_config = None;
        env.ndm_config_dir = None;
        env.xdg_config_home = None;
        let resolved = resolve_paths(&cli, &env).unwrap();
        assert_eq!(
            resolved.main_config,
            PathBuf::from("/home/user/.config/niri/config.kdl")
        );
    }

    #[test]
    fn missing_base_environment_is_a_typed_error() {
        let cli = CliOptions::default();
        let error = resolve_paths(&cli, &Environment::default()).unwrap_err();
        assert!(matches!(error, ConfigError::NoConfigBase));
        assert!(error.to_string().contains("HOME"));
    }

    #[test]
    fn explicit_display_config_is_flagged_and_absolute() {
        let cli = CliOptions {
            display_config: Some(PathBuf::from("relative/display.kdl")),
            ..CliOptions::default()
        };
        let resolved = resolve_paths(&cli, &environment()).unwrap();
        assert!(resolved.display_explicit);
        assert!(resolved.display_candidate.is_absolute());
        assert!(resolved.display_candidate.ends_with("relative/display.kdl"));
    }

    #[test]
    fn default_display_candidate_follows_the_main_config_directory() {
        let cli = CliOptions::default();
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("/xdg/config")),
            ..Environment::default()
        };
        let resolved = resolve_paths(&cli, &env).unwrap();
        assert_eq!(
            resolved.display_candidate,
            PathBuf::from("/xdg/config/niri/cfg/display.kdl")
        );
    }
}
