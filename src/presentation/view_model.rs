//! Presentation view model: pure state and row/text builders.
//!
//! Mapping application state to UI content lives here as plain data, so it can
//! be unit-tested without a running display server. All user-visible strings
//! are ASCII technical text: no emoji, matching the project UI policy.

use crate::domain::display::{ConnectorStatus, DisplayOutput, format_scale};
use crate::domain::profile::ProfileKind;
use crate::service::backup_service::ConfigMode;
use crate::service::display_service::{ApplyReport, SystemState};

/// Actions dispatched from the UI to the worker thread.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Refresh,
    ApplyProfile(ProfileKind),
    StopMirror,
    Shutdown,
}

/// Events sent from the worker thread back to the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    State(Box<SystemState>),
    Applied(Box<ApplyReport>),
    Failed(String),
    Notice(String),
    Busy(bool),
    ShutdownComplete,
}

/// Aggregated UI state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppState {
    pub outputs: Vec<DisplayOutput>,
    pub connectors: Vec<ConnectorStatus>,
    pub active_profile: Option<ProfileKind>,
    pub mirror_running: bool,
    pub config_mode: ConfigMode,
    pub display_file_registered: bool,
    pub busy: bool,
}

impl AppState {
    pub fn from_system(state: SystemState) -> Self {
        Self {
            outputs: state.outputs,
            connectors: state.connectors,
            active_profile: state.active_profile,
            mirror_running: state.mirror_running,
            config_mode: state.config_mode,
            display_file_registered: state.display_file_registered,
            busy: false,
        }
    }
}

/// One profile entry rendered as an `AdwActionRow`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRow {
    pub profile: ProfileKind,
    pub title: String,
    pub subtitle: String,
    pub icon_name: String,
    pub active: bool,
    pub sensitive: bool,
}

/// A generic information row (outputs, ports).
#[derive(Debug, Clone, PartialEq)]
pub struct InfoRow {
    pub title: String,
    pub subtitle: String,
    pub icon_name: String,
}

/// Build the five profile rows for the current state.
pub fn profile_rows(state: &AppState) -> Vec<ProfileRow> {
    ProfileKind::ALL
        .into_iter()
        .map(|profile| {
            let subtitle = if profile == ProfileKind::Mirror && state.mirror_running {
                "Mirroring is currently active".to_owned()
            } else {
                profile.description().to_owned()
            };
            ProfileRow {
                profile,
                title: profile.label().to_owned(),
                subtitle,
                icon_name: profile.icon_name().to_owned(),
                active: state.active_profile == Some(profile) && !state.busy,
                sensitive: !state.busy,
            }
        })
        .collect()
}

/// Build one row per niri output.
pub fn output_rows(state: &AppState) -> Vec<InfoRow> {
    state
        .outputs
        .iter()
        .map(|output| InfoRow {
            title: output.name.clone(),
            subtitle: output_subtitle(output),
            icon_name: if output.is_internal() {
                "computer-symbolic".to_owned()
            } else {
                "video-display-symbolic".to_owned()
            },
        })
        .collect()
}

/// Build one row per DRM connector.
pub fn connector_rows(state: &AppState) -> Vec<InfoRow> {
    state
        .connectors
        .iter()
        .map(|connector| InfoRow {
            title: connector.connector.clone(),
            subtitle: format!(
                "{} - {}",
                connector.card,
                if connector.connected {
                    "connected"
                } else {
                    "disconnected"
                }
            ),
            icon_name: "video-display-symbolic".to_owned(),
        })
        .collect()
}

/// One-line status summary for the status row.
pub fn status_summary(state: &AppState) -> String {
    let mut summary = if state.busy {
        "Applying changes...".to_owned()
    } else if state.mirror_running {
        "Mirroring is active".to_owned()
    } else if let Some(profile) = state.active_profile {
        format!("Active profile: {}", profile.label())
    } else {
        "No managed profile applied".to_owned()
    };
    match state.config_mode {
        ConfigMode::Inline => summary.push_str(" - portable mode (fenced section in config.kdl)"),
        ConfigMode::Modular if !state.display_file_registered => {
            summary.push_str(" - setup required: the display file is not included by config.kdl")
        }
        ConfigMode::Modular => summary.push_str(" - modular mode (cfg/display.kdl)"),
    }
    summary
}

fn output_subtitle(output: &DisplayOutput) -> String {
    let mut parts = vec![output.describe_mode()];
    match &output.logical {
        Some(logical) => {
            parts.push(format!("position {},{}", logical.x, logical.y));
            parts.push(format!("scale {}", format_scale(logical.scale)));
        }
        None => parts.push("disabled".to_owned()),
    }
    parts.join(" - ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::{DisplayMode, LogicalGeometry};

    fn output(name: &str, enabled: bool) -> DisplayOutput {
        DisplayOutput {
            name: name.to_owned(),
            make: "Test".to_owned(),
            model: "Panel".to_owned(),
            serial: None,
            physical_size: Some((300, 200)),
            modes: vec![DisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate: 144001,
                is_preferred: true,
            }],
            current_mode: Some(0),
            vrr_supported: false,
            vrr_enabled: false,
            logical: enabled.then(|| LogicalGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
                transform: "normal".to_owned(),
            }),
        }
    }

    fn state() -> AppState {
        AppState {
            outputs: vec![output("eDP-1", true), output("HDMI-A-1", false)],
            connectors: vec![
                ConnectorStatus {
                    card: "card1".to_owned(),
                    connector: "eDP-1".to_owned(),
                    connected: true,
                },
                ConnectorStatus {
                    card: "card1".to_owned(),
                    connector: "HDMI-A-1".to_owned(),
                    connected: false,
                },
            ],
            active_profile: Some(ProfileKind::ExtendRight),
            mirror_running: false,
            config_mode: ConfigMode::Modular,
            display_file_registered: true,
            busy: false,
        }
    }

    #[test]
    fn profile_rows_mark_the_active_profile_and_respect_busy_state() {
        let rows = profile_rows(&state());
        assert_eq!(rows.len(), 5);
        let active: Vec<&ProfileRow> = rows.iter().filter(|row| row.active).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].profile, ProfileKind::ExtendRight);

        let mut busy = state();
        busy.busy = true;
        let rows = profile_rows(&busy);
        assert!(rows.iter().all(|row| !row.active && !row.sensitive));
    }

    #[test]
    fn output_rows_describe_mode_position_and_scale() {
        let rows = output_rows(&state());
        assert_eq!(rows[0].title, "eDP-1");
        assert!(rows[0].subtitle.contains("1920x1080 @ 144.001 Hz"));
        assert!(rows[0].subtitle.contains("position 0,0"));
        assert!(rows[0].subtitle.contains("scale 1"));
        assert!(rows[1].subtitle.contains("disabled"));
    }

    #[test]
    fn connector_rows_report_card_and_cable_state() {
        let rows = connector_rows(&state());
        assert_eq!(rows[0].subtitle, "card1 - connected");
        assert_eq!(rows[1].subtitle, "card1 - disconnected");
    }

    #[test]
    fn status_summary_covers_busy_mirror_profile_and_modes() {
        let mut current = state();
        assert_eq!(
            status_summary(&current),
            "Active profile: Extend Right - modular mode (cfg/display.kdl)"
        );

        current.mirror_running = true;
        assert_eq!(
            status_summary(&current),
            "Mirroring is active - modular mode (cfg/display.kdl)"
        );

        current.busy = true;
        assert_eq!(
            status_summary(&current),
            "Applying changes... - modular mode (cfg/display.kdl)"
        );

        current.busy = false;
        current.active_profile = None;
        current.mirror_running = false;
        current.config_mode = ConfigMode::Inline;
        assert_eq!(
            status_summary(&current),
            "No managed profile applied - portable mode (fenced section in config.kdl)"
        );

        current.config_mode = ConfigMode::Modular;
        current.display_file_registered = false;
        assert_eq!(
            status_summary(&current),
            "No managed profile applied - setup required: the display file is not included by config.kdl"
        );
    }

    #[test]
    fn every_visible_string_is_ascii() {
        let state = state();
        let mut strings = Vec::new();
        for row in profile_rows(&state) {
            strings.push(row.title);
            strings.push(row.subtitle);
            strings.push(row.icon_name);
        }
        for row in output_rows(&state) {
            strings.push(row.title);
            strings.push(row.subtitle);
            strings.push(row.icon_name);
        }
        for row in connector_rows(&state) {
            strings.push(row.title);
            strings.push(row.subtitle);
            strings.push(row.icon_name);
        }
        strings.push(status_summary(&state));
        for value in strings {
            assert!(value.is_ascii(), "non-ASCII UI string: {value}");
        }
    }
}
