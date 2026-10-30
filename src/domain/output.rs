//! Per-output configuration types.
//!
//! These types capture the settings a user can change for a single output
//! (mode, scale, transform, VRR) and are shared by planning, KDL rendering,
//! verification, and the UI. They are pure data with no I/O.

use super::display::{DisplayMode, DisplayOutput};

/// Exact display mode: resolution plus refresh rate in millihertz.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModeSpec {
    pub width: u32,
    pub height: u32,
    /// Refresh rate in millihertz, matching niri's JSON representation.
    pub refresh_millihertz: u32,
}

impl ModeSpec {
    pub fn from_mode(mode: &DisplayMode) -> Self {
        Self {
            width: mode.width,
            height: mode.height,
            refresh_millihertz: mode.refresh_rate,
        }
    }

    pub fn refresh_hz(&self) -> String {
        refresh_to_hz(self.refresh_millihertz)
    }

    /// Human-readable label, for example `1920x1080 @ 144.001 Hz`.
    pub fn label(&self) -> String {
        format!("{}x{} @ {} Hz", self.width, self.height, self.refresh_hz())
    }

    /// Value used inside the niri `mode "..."` directive.
    pub fn kdl_value(&self) -> String {
        format!("{}x{}@{}", self.width, self.height, self.refresh_hz())
    }

    pub fn matches(&self, mode: &DisplayMode) -> bool {
        self.width == mode.width
            && self.height == mode.height
            && self.refresh_millihertz == mode.refresh_rate
    }

    /// Parse a `WxH@Hz` value as written by this manager or `niri msg`.
    pub fn from_kdl(value: &str) -> Option<Self> {
        let (size, refresh) = value.split_once('@')?;
        let (width, height) = size.split_once('x')?;
        let width: u32 = width.parse().ok()?;
        let height: u32 = height.parse().ok()?;
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            width,
            height,
            refresh_millihertz: parse_refresh_millihertz(refresh)?,
        })
    }
}

/// Format millihertz with exactly three decimals, matching niri's mode syntax.
pub fn refresh_to_hz(refresh_millihertz: u32) -> String {
    format!(
        "{}.{:03}",
        refresh_millihertz / 1000,
        refresh_millihertz % 1000
    )
}

fn parse_refresh_millihertz(value: &str) -> Option<u32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let mut digits: String = fraction.chars().take(3).collect();
    while digits.len() < 3 {
        digits.push('0');
    }
    let fraction: u32 = digits.parse().ok()?;
    whole.checked_mul(1000)?.checked_add(fraction)
}

/// Output rotation/flip as accepted by niri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    pub const ALL: [Transform; 8] = [
        Transform::Normal,
        Transform::Rotate90,
        Transform::Rotate180,
        Transform::Rotate270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    pub fn niri_value(self) -> &'static str {
        match self {
            Transform::Normal => "normal",
            Transform::Rotate90 => "90",
            Transform::Rotate180 => "180",
            Transform::Rotate270 => "270",
            Transform::Flipped => "flipped",
            Transform::Flipped90 => "flipped-90",
            Transform::Flipped180 => "flipped-180",
            Transform::Flipped270 => "flipped-270",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Transform::Normal => "Normal",
            Transform::Rotate90 => "Rotate 90 degrees",
            Transform::Rotate180 => "Rotate 180 degrees",
            Transform::Rotate270 => "Rotate 270 degrees",
            Transform::Flipped => "Flipped",
            Transform::Flipped90 => "Flipped 90 degrees",
            Transform::Flipped180 => "Flipped 180 degrees",
            Transform::Flipped270 => "Flipped 270 degrees",
        }
    }

    pub fn from_niri(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|transform| transform.niri_value() == normalized)
    }

    pub fn index(self) -> u32 {
        Self::ALL
            .iter()
            .position(|transform| *transform == self)
            .unwrap_or(0) as u32
    }

    pub fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }
}

/// Variable refresh rate configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VrrMode {
    #[default]
    Off,
    On,
    OnDemand,
}

impl VrrMode {
    pub const ALL: [VrrMode; 3] = [VrrMode::Off, VrrMode::On, VrrMode::OnDemand];

    pub fn label(self) -> &'static str {
        match self {
            VrrMode::Off => "Off",
            VrrMode::On => "On",
            VrrMode::OnDemand => "On demand",
        }
    }

    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0) as u32
    }

    pub fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or_default()
    }

    /// Whether the manager should emit the `variable-refresh-rate` flag.
    pub fn is_enabled(self) -> bool {
        !matches!(self, VrrMode::Off)
    }
}

/// Persisted per-output preferences recovered from the managed KDL fence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OutputPreferences {
    pub mode: Option<ModeSpec>,
    pub scale: Option<f64>,
    pub transform: Option<Transform>,
    pub vrr: Option<VrrMode>,
}

impl OutputPreferences {
    /// Derive preferences from the live compositor state.
    pub fn from_output(output: &DisplayOutput) -> Self {
        let mode = output.current_mode().map(ModeSpec::from_mode);
        let scale = output.logical.as_ref().map(|logical| logical.scale);
        let transform = output
            .logical
            .as_ref()
            .and_then(|logical| Transform::from_niri(&logical.transform))
            .filter(|transform| *transform != Transform::Normal);
        let vrr = if output.vrr_supported {
            Some(if output.vrr_enabled {
                VrrMode::On
            } else {
                VrrMode::Off
            })
        } else {
            None
        };
        Self {
            mode,
            scale,
            transform,
            vrr,
        }
    }
}

/// Requested changes for one output, emitted by the per-monitor UI.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputUpdate {
    pub enabled: bool,
    pub mode: Option<ModeSpec>,
    pub scale: Option<f64>,
    pub transform: Option<Transform>,
    pub position: Option<(i32, i32)>,
    pub vrr: Option<VrrMode>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::display::{DisplayMode, LogicalGeometry};

    fn output() -> DisplayOutput {
        DisplayOutput {
            name: "eDP-1".to_owned(),
            make: "Test".to_owned(),
            model: "Panel".to_owned(),
            serial: None,
            physical_size: Some((340, 190)),
            modes: vec![DisplayMode {
                width: 1920,
                height: 1080,
                refresh_rate: 144_001,
                is_preferred: true,
            }],
            current_mode: Some(0),
            vrr_supported: true,
            vrr_enabled: true,
            logical: Some(LogicalGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.25,
                transform: "90".to_owned(),
            }),
        }
    }

    #[test]
    fn mode_spec_round_trips_through_kdl() {
        let spec = ModeSpec {
            width: 1920,
            height: 1080,
            refresh_millihertz: 144_001,
        };
        assert_eq!(spec.kdl_value(), "1920x1080@144.001");
        assert_eq!(ModeSpec::from_kdl("1920x1080@144.001"), Some(spec.clone()));
        assert_eq!(spec.label(), "1920x1080 @ 144.001 Hz");

        assert_eq!(
            ModeSpec::from_kdl("2560x1440@59.951"),
            Some(ModeSpec {
                width: 2560,
                height: 1440,
                refresh_millihertz: 59_951
            })
        );
        assert_eq!(
            ModeSpec::from_kdl("1920x1080@60"),
            Some(ModeSpec {
                width: 1920,
                height: 1080,
                refresh_millihertz: 60_000
            })
        );
        assert_eq!(ModeSpec::from_kdl("garbage"), None);
        assert_eq!(ModeSpec::from_kdl("0x1080@60"), None);
        assert_eq!(ModeSpec::from_kdl("1920x1080@6x"), None);
    }

    #[test]
    fn transforms_parse_and_index_round_trip() {
        for transform in Transform::ALL {
            assert_eq!(
                Transform::from_niri(transform.niri_value()),
                Some(transform)
            );
            assert_eq!(Transform::from_index(transform.index()), transform);
            assert!(transform.label().is_ascii());
        }
        assert_eq!(Transform::from_niri("Normal"), Some(Transform::Normal));
        assert_eq!(Transform::from_niri("unknown"), None);
    }

    #[test]
    fn vrr_modes_index_round_trip() {
        for mode in VrrMode::ALL {
            assert_eq!(VrrMode::from_index(mode.index()), mode);
            assert!(mode.label().is_ascii());
        }
        assert!(!VrrMode::Off.is_enabled());
        assert!(VrrMode::OnDemand.is_enabled());
    }

    #[test]
    fn preferences_are_derived_from_live_state() {
        let preferences = OutputPreferences::from_output(&output());
        assert_eq!(
            preferences.mode,
            Some(ModeSpec {
                width: 1920,
                height: 1080,
                refresh_millihertz: 144_001
            })
        );
        assert_eq!(preferences.scale, Some(1.25));
        assert_eq!(preferences.transform, Some(Transform::Rotate90));
        assert_eq!(preferences.vrr, Some(VrrMode::On));

        let mut disabled = output();
        disabled.logical = None;
        disabled.current_mode = None;
        let preferences = OutputPreferences::from_output(&disabled);
        assert_eq!(preferences.mode, None);
        assert_eq!(preferences.scale, None);
        assert_eq!(preferences.transform, None);
    }

    #[test]
    fn refresh_formatting_is_exact() {
        assert_eq!(refresh_to_hz(144_001), "144.001");
        assert_eq!(refresh_to_hz(60_000), "60.000");
        assert_eq!(refresh_to_hz(59_951), "59.951");
    }
}
