//! Display profiles and pure layout planning.
//!
//! A profile is a named intent (internal only, extend right, extend left,
//! mirror, external only). Planning turns a profile plus observed outputs and
//! DRM connectors into a concrete [`LayoutPlan`] and, for mirroring, a
//! [`MirrorPlan`]. Both functions are pure and fully unit-testable.

use super::display::{
    ConnectorKind, ConnectorStatus, DEFAULT_LOGICAL_WIDTH, DisplayOutput, OutputId,
};
use std::fmt;

/// The five supported display profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileKind {
    InternalOnly,
    ExtendRight,
    ExtendLeft,
    Mirror,
    ExternalOnly,
}

impl ProfileKind {
    /// All profiles in presentation order.
    pub const ALL: [ProfileKind; 5] = [
        ProfileKind::InternalOnly,
        ProfileKind::ExtendRight,
        ProfileKind::ExtendLeft,
        ProfileKind::Mirror,
        ProfileKind::ExternalOnly,
    ];

    /// Stable identifier used in the managed KDL section and logs.
    pub fn id(self) -> &'static str {
        match self {
            ProfileKind::InternalOnly => "internal-only",
            ProfileKind::ExtendRight => "extend-right",
            ProfileKind::ExtendLeft => "extend-left",
            ProfileKind::Mirror => "mirror",
            ProfileKind::ExternalOnly => "external-only",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            ProfileKind::InternalOnly => "Internal Display Only",
            ProfileKind::ExtendRight => "Extend Right",
            ProfileKind::ExtendLeft => "Extend Left",
            ProfileKind::Mirror => "Mirror / Presentation",
            ProfileKind::ExternalOnly => "External Only",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ProfileKind::InternalOnly => "Restore the laptop panel and stop mirroring",
            ProfileKind::ExtendRight => {
                "Place the external display to the right of the laptop panel"
            }
            ProfileKind::ExtendLeft => "Place the external display to the left of the laptop panel",
            ProfileKind::Mirror => "Mirror the laptop panel to a projector or TV over HDMI",
            ProfileKind::ExternalOnly => {
                "Turn off the laptop panel and use the external display only"
            }
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            ProfileKind::InternalOnly => "video-single-display-symbolic",
            ProfileKind::ExtendRight | ProfileKind::ExtendLeft => "video-joined-displays-symbolic",
            ProfileKind::Mirror => "display-projector-symbolic",
            ProfileKind::ExternalOnly => "video-display-symbolic",
        }
    }
}

impl fmt::Display for ProfileKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// Desired state for one output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPlan {
    pub output: OutputId,
    pub enabled: bool,
    /// Absolute logical position when the output is enabled.
    pub position: Option<(i32, i32)>,
    /// Explicit scale when the profile needs deterministic geometry.
    pub scale: Option<f64>,
    /// Whether verification requires the output to be visible in niri.
    ///
    /// Extend profiles persist a plan for a not-yet-connected display, so that
    /// block must not fail verification while the cable is absent.
    pub expect_present: bool,
}

/// Complete layout intent for a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutPlan {
    pub profile: ProfileKind,
    pub outputs: Vec<OutputPlan>,
    /// Output that should receive focus after the profile is applied.
    pub focus_output: Option<OutputId>,
}

/// Mirroring intent: replicate `source` onto `target` using wl-mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorPlan {
    pub source: OutputId,
    pub target: OutputId,
}

impl MirrorPlan {
    pub fn program() -> &'static str {
        "wl-mirror"
    }

    /// Arguments for `wl-mirror`.
    ///
    /// `--fullscreen-output` makes wl-mirror request fullscreen on the target
    /// output itself, which avoids fragile window-moving choreography.
    pub fn command_args(&self) -> Vec<String> {
        vec![
            "--fullscreen-output".to_owned(),
            self.target.as_str().to_owned(),
            self.source.as_str().to_owned(),
        ]
    }
}

/// Errors raised while planning a profile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("no internal display (eDP/LVDS/DSI) is available")]
    NoInternalDisplay,
    #[error("no external display port (HDMI/DP) is available")]
    NoExternalPort,
    #[error("no external display is connected; attach a display and refresh")]
    ExternalNotConnected,
    #[error("the external display is not available in niri yet; refresh and retry")]
    ExternalNotReady,
    #[error("mirroring requires both an internal panel and an available external display")]
    MirrorRequiresTwoDisplays,
}

/// Extend direction for position calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtendDirection {
    Right,
    Left,
}

/// Plan a layout for `profile` from observed outputs and DRM connectors.
pub fn plan_profile(
    profile: ProfileKind,
    outputs: &[DisplayOutput],
    connectors: &[ConnectorStatus],
) -> Result<LayoutPlan, PlanError> {
    match profile {
        ProfileKind::InternalOnly => Ok(LayoutPlan {
            profile,
            outputs: Vec::new(),
            focus_output: None,
        }),
        ProfileKind::ExtendRight => {
            plan_extend(profile, outputs, connectors, ExtendDirection::Right)
        }
        ProfileKind::ExtendLeft => plan_extend(profile, outputs, connectors, ExtendDirection::Left),
        ProfileKind::Mirror => plan_mirror_layout(outputs),
        ProfileKind::ExternalOnly => plan_external_only(outputs, connectors),
    }
}

/// Plan the wl-mirror invocation for the mirror profile.
///
/// The source is always the internal panel; the target is the best external
/// display. Both must be enabled in niri before mirroring starts.
pub fn plan_mirror(outputs: &[DisplayOutput]) -> Result<MirrorPlan, PlanError> {
    let internal = internal_output(outputs).ok_or(PlanError::NoInternalDisplay)?;
    let external = match external_output(outputs) {
        Some(external) if external.is_enabled() => external,
        Some(_) => return Err(PlanError::ExternalNotReady),
        None => return Err(PlanError::MirrorRequiresTwoDisplays),
    };
    Ok(MirrorPlan {
        source: internal.id(),
        target: external.id(),
    })
}

/// Check observed outputs against a plan; returns a mismatch description.
pub fn verify_plan(plan: &LayoutPlan, outputs: &[DisplayOutput]) -> Result<(), String> {
    for expected in &plan.outputs {
        let actual = outputs
            .iter()
            .find(|output| output.name == expected.output.as_str());
        let Some(actual) = actual else {
            if expected.expect_present {
                return Err(format!(
                    "output {} is missing from the compositor",
                    expected.output
                ));
            }
            continue;
        };
        if expected.enabled {
            let Some(logical) = &actual.logical else {
                return Err(format!("output {} is still disabled", expected.output));
            };
            if let Some((x, y)) = expected.position
                && (logical.x != x || logical.y != y)
            {
                return Err(format!(
                    "output {} is at ({}, {}), expected ({}, {})",
                    expected.output, logical.x, logical.y, x, y
                ));
            }
        } else if actual.logical.is_some() {
            return Err(format!("output {} is still enabled", expected.output));
        }
    }
    Ok(())
}

fn internal_output(outputs: &[DisplayOutput]) -> Option<&DisplayOutput> {
    outputs.iter().find(|output| output.is_internal())
}

/// Pick the best external output deterministically: HDMI first, then enabled,
/// then by name.
fn external_output(outputs: &[DisplayOutput]) -> Option<&DisplayOutput> {
    outputs
        .iter()
        .filter(|output| !output.is_internal())
        .min_by_key(|output| external_preference(&output.name, output.is_enabled()))
}

/// Pick the best external connector deterministically when niri does not list
/// the display yet (for example a disconnected HDMI port).
fn external_connector(connectors: &[ConnectorStatus]) -> Option<&ConnectorStatus> {
    connectors
        .iter()
        .filter(|connector| matches!(connector.kind(), ConnectorKind::External))
        .min_by_key(|connector| {
            let hdmi = u8::from(!connector.connector.to_ascii_uppercase().starts_with("HDMI"));
            let disconnected = u8::from(!connector.connected);
            (
                hdmi,
                disconnected,
                connector.card.clone(),
                connector.connector.to_ascii_lowercase(),
            )
        })
}

fn external_preference(name: &str, enabled: bool) -> (u8, u8, String) {
    let hdmi = u8::from(!name.to_ascii_uppercase().starts_with("HDMI"));
    let disabled = u8::from(!enabled);
    (hdmi, disabled, name.to_ascii_lowercase())
}

fn plan_extend(
    profile: ProfileKind,
    outputs: &[DisplayOutput],
    connectors: &[ConnectorStatus],
    direction: ExtendDirection,
) -> Result<LayoutPlan, PlanError> {
    let internal = internal_output(outputs).ok_or(PlanError::NoInternalDisplay)?;
    let internal_id = internal.id();
    let internal_width = internal.reference_logical_width();

    let (external_id, external_present) = match external_output(outputs) {
        Some(external) => (external.id(), true),
        None => {
            let connector = external_connector(connectors).ok_or(PlanError::NoExternalPort)?;
            (connector.id(), false)
        }
    };

    let external_width = outputs
        .iter()
        .find(|output| output.name == external_id.as_str())
        .map(DisplayOutput::reference_logical_width)
        .unwrap_or(DEFAULT_LOGICAL_WIDTH);

    let external_x = match direction {
        ExtendDirection::Right => internal_width,
        ExtendDirection::Left => -external_width,
    };

    Ok(LayoutPlan {
        profile,
        outputs: vec![
            OutputPlan {
                output: internal_id,
                enabled: true,
                position: Some((0, 0)),
                scale: None,
                expect_present: true,
            },
            OutputPlan {
                output: external_id,
                enabled: true,
                position: Some((external_x, 0)),
                scale: Some(1.0),
                expect_present: external_present,
            },
        ],
        focus_output: None,
    })
}

fn plan_external_only(
    outputs: &[DisplayOutput],
    connectors: &[ConnectorStatus],
) -> Result<LayoutPlan, PlanError> {
    let internal = internal_output(outputs).ok_or(PlanError::NoInternalDisplay)?;
    let external = external_output(outputs);
    let external = match external {
        Some(external) if external.is_enabled() => external,
        Some(_) => return Err(PlanError::ExternalNotReady),
        None => {
            let external_connectors: Vec<&ConnectorStatus> = connectors
                .iter()
                .filter(|connector| matches!(connector.kind(), ConnectorKind::External))
                .collect();
            if external_connectors
                .iter()
                .any(|connector| connector.connected)
            {
                return Err(PlanError::ExternalNotReady);
            }
            if external_connectors.is_empty() {
                return Err(PlanError::NoExternalPort);
            }
            return Err(PlanError::ExternalNotConnected);
        }
    };

    Ok(LayoutPlan {
        profile: ProfileKind::ExternalOnly,
        outputs: vec![
            OutputPlan {
                output: internal.id(),
                enabled: false,
                position: None,
                scale: None,
                expect_present: true,
            },
            OutputPlan {
                output: external.id(),
                enabled: true,
                position: Some((0, 0)),
                scale: Some(1.0),
                expect_present: true,
            },
        ],
        focus_output: Some(external.id()),
    })
}

fn plan_mirror_layout(outputs: &[DisplayOutput]) -> Result<LayoutPlan, PlanError> {
    let internal = internal_output(outputs).ok_or(PlanError::NoInternalDisplay)?;
    let external = match external_output(outputs) {
        Some(external) if external.is_enabled() => external,
        Some(_) => return Err(PlanError::ExternalNotReady),
        None => return Err(PlanError::MirrorRequiresTwoDisplays),
    };

    let internal_width = internal.reference_logical_width();

    Ok(LayoutPlan {
        profile: ProfileKind::Mirror,
        outputs: vec![
            OutputPlan {
                output: internal.id(),
                enabled: true,
                position: Some((0, 0)),
                scale: None,
                expect_present: true,
            },
            OutputPlan {
                output: external.id(),
                enabled: true,
                position: Some((internal_width, 0)),
                scale: Some(1.0),
                expect_present: true,
            },
        ],
        focus_output: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::DisplayMode;

    fn enabled_output(name: &str, logical_width: i32) -> DisplayOutput {
        DisplayOutput {
            name: name.to_owned(),
            make: "Test".to_owned(),
            model: "Panel".to_owned(),
            serial: None,
            physical_size: Some((300, 200)),
            modes: vec![DisplayMode {
                width: logical_width as u32,
                height: 1080,
                refresh_rate: 60001,
                is_preferred: true,
            }],
            current_mode: Some(0),
            vrr_supported: false,
            vrr_enabled: false,
            logical: Some(crate::domain::display::LogicalGeometry {
                x: 0,
                y: 0,
                width: logical_width,
                height: 1080,
                scale: 1.0,
                transform: "normal".to_owned(),
            }),
        }
    }

    fn disabled_output(name: &str) -> DisplayOutput {
        let mut output = enabled_output(name, 3840);
        output.logical = None;
        output.current_mode = None;
        output
    }

    fn connector(name: &str, card: &str, connected: bool) -> ConnectorStatus {
        ConnectorStatus {
            card: card.to_owned(),
            connector: name.to_owned(),
            connected,
        }
    }

    fn dual_outputs() -> Vec<DisplayOutput> {
        vec![
            enabled_output("eDP-1", 1920),
            enabled_output("HDMI-A-1", 2560),
        ]
    }

    #[test]
    fn extend_right_places_external_after_internal() {
        let plan = plan_profile(ProfileKind::ExtendRight, &dual_outputs(), &[]).unwrap();
        assert_eq!(plan.outputs.len(), 2);
        assert_eq!(plan.outputs[0].position, Some((0, 0)));
        assert_eq!(plan.outputs[1].output.as_str(), "HDMI-A-1");
        assert_eq!(plan.outputs[1].position, Some((1920, 0)));
        assert_eq!(plan.outputs[1].scale, Some(1.0));
        assert!(plan.outputs[1].expect_present);
    }

    #[test]
    fn extend_left_places_external_before_internal() {
        let plan = plan_profile(ProfileKind::ExtendLeft, &dual_outputs(), &[]).unwrap();
        assert_eq!(plan.outputs[1].position, Some((-2560, 0)));
    }

    #[test]
    fn extend_without_connected_display_persists_non_verifiable_block() {
        let outputs = vec![enabled_output("eDP-1", 1920)];
        let connectors = vec![connector("HDMI-A-1", "card1", false)];
        let plan = plan_profile(ProfileKind::ExtendRight, &outputs, &connectors).unwrap();
        assert_eq!(plan.outputs.len(), 2);
        assert_eq!(plan.outputs[1].output.as_str(), "HDMI-A-1");
        assert_eq!(plan.outputs[1].position, Some((1920, 0)));
        assert!(!plan.outputs[1].expect_present);
    }

    #[test]
    fn extend_without_any_external_port_fails() {
        let outputs = vec![enabled_output("eDP-1", 1920)];
        let error = plan_profile(ProfileKind::ExtendRight, &outputs, &[]).unwrap_err();
        assert_eq!(error, PlanError::NoExternalPort);
    }

    #[test]
    fn external_only_disables_internal_and_focuses_external() {
        let plan = plan_profile(ProfileKind::ExternalOnly, &dual_outputs(), &[]).unwrap();
        assert!(!plan.outputs[0].enabled);
        assert!(plan.outputs[1].enabled);
        assert_eq!(plan.outputs[1].position, Some((0, 0)));
        assert_eq!(
            plan.focus_output.as_ref().map(OutputId::as_str),
            Some("HDMI-A-1")
        );
    }

    #[test]
    fn external_only_requires_connected_external_display() {
        let outputs = vec![enabled_output("eDP-1", 1920)];
        let connectors = vec![connector("HDMI-A-1", "card1", false)];
        let error = plan_profile(ProfileKind::ExternalOnly, &outputs, &connectors).unwrap_err();
        assert_eq!(error, PlanError::ExternalNotConnected);
    }

    #[test]
    fn external_only_rejects_present_but_disabled_external_output() {
        let outputs = vec![enabled_output("eDP-1", 1920), disabled_output("HDMI-A-1")];
        let error = plan_profile(ProfileKind::ExternalOnly, &outputs, &[]).unwrap_err();
        assert_eq!(error, PlanError::ExternalNotReady);
    }

    #[test]
    fn mirror_plan_targets_best_external_and_builds_wl_mirror_arguments() {
        let mirror = plan_mirror(&dual_outputs()).unwrap();
        assert_eq!(mirror.source.as_str(), "eDP-1");
        assert_eq!(mirror.target.as_str(), "HDMI-A-1");
        assert_eq!(
            mirror.command_args(),
            vec!["--fullscreen-output", "HDMI-A-1", "eDP-1"]
        );
    }

    #[test]
    fn mirror_requires_available_external_display() {
        let outputs = vec![enabled_output("eDP-1", 1920), disabled_output("HDMI-A-1")];
        let error = plan_mirror(&outputs).unwrap_err();
        assert_eq!(error, PlanError::ExternalNotReady);
    }

    #[test]
    fn verification_detects_missing_disabled_and_misplaced_outputs() {
        let plan = plan_profile(ProfileKind::ExtendRight, &dual_outputs(), &[]).unwrap();

        let mut misplaced = dual_outputs();
        if let Some(logical) = misplaced[1].logical.as_mut() {
            logical.x = 999;
        }
        let error = verify_plan(&plan, &misplaced).unwrap_err();
        assert!(
            error.contains("expected (1920, 0)"),
            "unexpected error: {error}"
        );

        let mut still_off = dual_outputs();
        still_off[1].logical = None;
        let error = verify_plan(&plan, &still_off).unwrap_err();
        assert!(
            error.contains("still disabled"),
            "unexpected error: {error}"
        );

        let error = verify_plan(&plan, &[enabled_output("eDP-1", 1920)]).unwrap_err();
        assert!(error.contains("missing"), "unexpected error: {error}");
    }

    #[test]
    fn verification_skips_absent_optional_outputs() {
        let outputs = vec![enabled_output("eDP-1", 1920)];
        let connectors = vec![connector("HDMI-A-1", "card1", false)];
        let plan = plan_profile(ProfileKind::ExtendRight, &outputs, &connectors).unwrap();
        assert!(verify_plan(&plan, &outputs).is_ok());
    }

    #[test]
    fn profile_ids_round_trip() {
        for profile in ProfileKind::ALL {
            assert_eq!(ProfileKind::from_id(profile.id()), Some(profile));
            assert!(!profile.label().is_empty());
            assert!(profile.label().is_ascii());
            assert!(profile.description().is_ascii());
        }
        assert_eq!(ProfileKind::from_id("nope"), None);
    }
}
