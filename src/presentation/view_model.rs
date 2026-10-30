//! Presentation view model: pure state and row/text builders.
//!
//! Mapping application state to UI content lives here as plain data, so it can
//! be unit-tested without a running display server. All user-visible strings
//! are ASCII technical text: no emoji, matching the project UI policy.

use std::collections::BTreeMap;

use crate::domain::display::{ConnectorStatus, DisplayMode, DisplayOutput, format_scale};
use crate::domain::output::{ModeSpec, OutputPreferences, OutputUpdate, Transform, VrrMode};
use crate::domain::profile::ProfileKind;
use crate::service::backup_service::ConfigMode;
use crate::service::display_service::{ApplyReport, SystemState};

/// Actions dispatched from the UI to the worker thread.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Refresh,
    ApplyProfile(ProfileKind),
    ApplyOutput {
        output: String,
        update: OutputUpdate,
    },
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
    pub output_preferences: BTreeMap<String, OutputPreferences>,
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
            output_preferences: state.output_preferences,
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
    if output.vrr_enabled {
        parts.push("VRR on".to_owned());
    }
    parts.join(" - ")
}

/// Summary shown in the collapsed output expander row.
pub fn output_summary(output: &DisplayOutput) -> String {
    output_subtitle(output)
}

/// Labels for the mode selector; the preferred mode is marked.
pub fn mode_labels(output: &DisplayOutput) -> Vec<String> {
    output
        .modes
        .iter()
        .map(|mode| {
            let label = format!("{}x{} @ {} Hz", mode.width, mode.height, mode.refresh_hz());
            if mode.is_preferred {
                format!("{label} (preferred)")
            } else {
                label
            }
        })
        .collect()
}

/// Index of the mode that should be selected initially.
pub fn selected_mode_index(output: &DisplayOutput) -> u32 {
    if let Some(index) = output.current_mode {
        return index as u32;
    }
    if let Some(index) = output.modes.iter().position(|mode| mode.is_preferred) {
        return index as u32;
    }
    0
}

/// Map a combo index back to a concrete mode.
pub fn mode_at(modes: &[DisplayMode], index: u32) -> Option<ModeSpec> {
    modes.get(index as usize).map(ModeSpec::from_mode)
}

/// Initial scale shown in the spin control.
pub fn initial_scale(output: &DisplayOutput) -> f64 {
    output
        .logical
        .as_ref()
        .map(|logical| logical.scale)
        .unwrap_or(1.0)
        .clamp(0.5, 3.0)
}

/// Initial transform combo index.
pub fn initial_transform_index(output: &DisplayOutput) -> u32 {
    output
        .logical
        .as_ref()
        .and_then(|logical| Transform::from_niri(&logical.transform))
        .unwrap_or_default()
        .index()
}

/// Initial VRR combo index, preferring the stored fence preference so that
/// `on-demand` survives a restart (JSON only exposes the enabled boolean).
pub fn initial_vrr_index(output: &DisplayOutput, preferences: Option<&OutputPreferences>) -> u32 {
    if !output.vrr_supported {
        return VrrMode::Off.index();
    }
    if let Some(mode) = preferences.and_then(|preferences| preferences.vrr) {
        return mode.index();
    }
    if output.vrr_enabled {
        VrrMode::On.index()
    } else {
        VrrMode::Off.index()
    }
}

/// Transform selector labels.
pub fn transform_labels() -> Vec<&'static str> {
    Transform::ALL
        .iter()
        .map(|transform| transform.label())
        .collect()
}

/// VRR selector labels.
pub fn vrr_labels() -> Vec<&'static str> {
    VrrMode::ALL.iter().map(|mode| mode.label()).collect()
}

/// Whether an output may be disabled without leaving zero active displays.
pub fn can_disable_output(state: &AppState, name: &str) -> bool {
    state
        .outputs
        .iter()
        .filter(|output| output.name != name && output.is_enabled())
        .count()
        > 0
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
            output_preferences: BTreeMap::new(),
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

    #[test]
    fn mode_helpers_mark_preferred_and_preselect_current() {
        let mut current = output("eDP-1", true);
        current.modes.push(DisplayMode {
            width: 1280,
            height: 720,
            refresh_rate: 60_000,
            is_preferred: false,
        });
        let labels = mode_labels(&current);
        assert_eq!(labels.len(), 2);
        assert!(labels[0].contains("(preferred)"));
        assert_eq!(selected_mode_index(&current), 0);
        assert_eq!(
            mode_at(&current.modes, 1),
            Some(ModeSpec {
                width: 1280,
                height: 720,
                refresh_millihertz: 60_000
            })
        );

        let mut disabled = output("HDMI-A-1", false);
        disabled.current_mode = None;
        assert_eq!(selected_mode_index(&disabled), 0);
        assert_eq!(mode_at(&disabled.modes, 99), None);
    }

    #[test]
    fn control_defaults_follow_live_and_stored_state() {
        let mut current = output("eDP-1", true);
        if let Some(logical) = current.logical.as_mut() {
            logical.scale = 1.25;
            logical.transform = "flipped-270".to_owned();
        }
        current.vrr_supported = true;
        current.vrr_enabled = true;
        assert_eq!(initial_scale(&current), 1.25);
        assert_eq!(
            initial_transform_index(&current),
            Transform::Flipped270.index()
        );
        assert_eq!(initial_vrr_index(&current, None), VrrMode::On.index());

        let stored = OutputPreferences {
            vrr: Some(VrrMode::OnDemand),
            ..OutputPreferences::default()
        };
        assert_eq!(
            initial_vrr_index(&current, Some(&stored)),
            VrrMode::OnDemand.index()
        );

        let unsupported = output("HDMI-A-1", true);
        assert_eq!(initial_vrr_index(&unsupported, None), VrrMode::Off.index());
        assert!(transform_labels().iter().all(|label| label.is_ascii()));
        assert!(vrr_labels().iter().all(|label| label.is_ascii()));
    }

    #[test]
    fn disabling_is_only_allowed_while_another_display_stays_enabled() {
        let mut current = state();
        // The external display is disabled in the fixture, so the panel must stay.
        assert!(!can_disable_output(&current, "eDP-1"));

        current.outputs[1].logical = Some(LogicalGeometry {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
            scale: 1.0,
            transform: "Normal".to_owned(),
        });
        assert!(can_disable_output(&current, "eDP-1"));
        assert!(can_disable_output(&current, "HDMI-A-1"));
    }
}
