//! DRM connector probing through `/sys/class/drm`.
//!
//! `niri msg --json outputs` only lists outputs the compositor knows about. A
//! physically connected cable whose output is disabled or not yet adopted is
//! only visible in sysfs, so the hardware probe is the ground truth for
//! "is a display attached to this port".

use std::fs;
use std::path::PathBuf;

use crate::domain::display::ConnectorStatus;

/// Hardware probing through the DRM sysfs interface.
pub trait HardwareProbe: Send + Sync {
    /// All DRM connectors with their cable connection state.
    fn connectors(&self) -> Result<Vec<ConnectorStatus>, HardwareError>;
}

/// Errors raised while reading DRM sysfs state.
#[derive(Debug, thiserror::Error)]
pub enum HardwareError {
    #[error("failed to read DRM directory {}: {source}", path.display())]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read DRM connector status {}: {source}", path.display())]
    ReadStatus {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Production probe reading `/sys/class/drm`.
#[derive(Debug, Clone)]
pub struct SysfsProbe {
    root: PathBuf,
}

impl SysfsProbe {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Probe the live system.
    pub fn system() -> Self {
        Self::new("/sys/class/drm")
    }
}

impl Default for SysfsProbe {
    fn default() -> Self {
        Self::system()
    }
}

impl HardwareProbe for SysfsProbe {
    fn connectors(&self) -> Result<Vec<ConnectorStatus>, HardwareError> {
        let entries = fs::read_dir(&self.root).map_err(|source| HardwareError::ReadDir {
            path: self.root.clone(),
            source,
        })?;

        let mut connectors = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| HardwareError::ReadDir {
                path: self.root.clone(),
                source,
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some((card, connector)) = parse_connector_entry(name) else {
                continue;
            };

            let status_path = entry.path().join("status");
            let status = match fs::read_to_string(&status_path) {
                Ok(status) => status,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(HardwareError::ReadStatus {
                        path: status_path,
                        source,
                    });
                }
            };

            connectors.push(ConnectorStatus {
                card,
                connector,
                connected: status.trim() == "connected",
            });
        }

        connectors.sort_by(|a, b| {
            a.card
                .cmp(&b.card)
                .then_with(|| a.connector.cmp(&b.connector))
        });
        Ok(connectors)
    }
}

/// Parse a sysfs entry name such as `card1-HDMI-A-1` into card and connector.
///
/// Returns `None` for anything that is not a `cardN-CONNECTOR` entry, which
/// filters out `card0`, `renderD128`, and the `version` file.
fn parse_connector_entry(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("card")?;
    let (card_number, connector) = rest.split_once('-')?;
    if card_number.is_empty()
        || connector.is_empty()
        || !card_number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((format!("card{card_number}"), connector.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_connector(root: &std::path::Path, entry: &str, status: &str) {
        let directory = root.join(entry);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("status"), status).unwrap();
    }

    #[test]
    fn parses_connector_entry_names() {
        assert_eq!(
            parse_connector_entry("card1-HDMI-A-1"),
            Some(("card1".to_owned(), "HDMI-A-1".to_owned()))
        );
        assert_eq!(
            parse_connector_entry("card0-eDP-2"),
            Some(("card0".to_owned(), "eDP-2".to_owned()))
        );
        assert_eq!(parse_connector_entry("card1"), None);
        assert_eq!(parse_connector_entry("renderD128"), None);
        assert_eq!(parse_connector_entry("cardX-HDMI-A-1"), None);
        assert_eq!(parse_connector_entry("card1-"), None);
        assert_eq!(parse_connector_entry("version"), None);
    }

    #[test]
    fn probes_fixture_directory_like_the_live_system() {
        let root = tempfile::tempdir().unwrap();
        write_connector(root.path(), "card0-HDMI-A-5", "disconnected\n");
        write_connector(root.path(), "card1-eDP-1", "connected\n");
        write_connector(root.path(), "card1-HDMI-A-1", "disconnected\n");
        fs::write(root.path().join("version"), "drm 1.0\n").unwrap();

        let probe = SysfsProbe::new(root.path());
        let connectors = probe.connectors().unwrap();
        assert_eq!(connectors.len(), 3);
        assert_eq!(connectors[0].card, "card0");
        assert_eq!(connectors[0].connector, "HDMI-A-5");
        assert!(!connectors[0].connected);
        assert_eq!(connectors[1].card, "card1");
        assert_eq!(connectors[1].connector, "HDMI-A-1");
        assert!(!connectors[1].connected);
        assert_eq!(connectors[2].connector, "eDP-1");
        assert!(connectors[2].connected);
    }

    #[test]
    fn missing_drm_root_is_an_error() {
        let probe = SysfsProbe::new("/nonexistent/drm");
        assert!(matches!(
            probe.connectors(),
            Err(HardwareError::ReadDir { .. })
        ));
    }
}
