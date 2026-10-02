//! Pure display entities.
//!
//! Values in this module mirror the subset of `niri msg --json outputs` that
//! the manager needs, plus DRM connector state read from sysfs. Nothing here
//! performs I/O or knows about the UI.

use serde::Deserialize;

/// Fallback logical width used when a disabled output advertises no mode.
pub const DEFAULT_LOGICAL_WIDTH: i32 = 1920;

/// Stable identifier for a compositor output, for example `eDP-1`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(String);

impl OutputId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OutputId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Classification derived from an output connector name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    Internal,
    External,
}

/// Classify an output by its connector prefix.
///
/// Internal panels use the `eDP`, `LVDS`, or `DSI` prefixes; everything else
/// (HDMI, DisplayPort, virtual outputs) is treated as external.
pub fn classify_connector(name: &str) -> ConnectorKind {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("edp") || lower.starts_with("lvds") || lower.starts_with("dsi") {
        ConnectorKind::Internal
    } else {
        ConnectorKind::External
    }
}

/// One mode advertised by an output (`niri msg --json outputs`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// Refresh rate in millihertz; `144001` means `144.001` Hz.
    pub refresh_rate: u32,
    #[serde(default)]
    pub is_preferred: bool,
}

impl DisplayMode {
    /// Human-readable refresh rate with three decimals, as niri reports it.
    pub fn refresh_hz(&self) -> String {
        format!(
            "{}.{:03}",
            self.refresh_rate / 1000,
            self.refresh_rate % 1000
        )
    }
}

/// Logical (post-scale) geometry of an enabled output.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LogicalGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    #[serde(default = "default_transform")]
    pub transform: String,
}

fn default_transform() -> String {
    "normal".to_owned()
}

/// A connected output as reported by niri.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DisplayOutput {
    pub name: String,
    #[serde(default)]
    pub make: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub physical_size: Option<(u32, u32)>,
    #[serde(default)]
    pub modes: Vec<DisplayMode>,
    #[serde(default)]
    pub current_mode: Option<usize>,
    #[serde(default)]
    pub vrr_supported: bool,
    #[serde(default)]
    pub vrr_enabled: bool,
    #[serde(default)]
    pub logical: Option<LogicalGeometry>,
}

impl DisplayOutput {
    pub fn id(&self) -> OutputId {
        OutputId::new(self.name.clone())
    }

    pub fn kind(&self) -> ConnectorKind {
        classify_connector(&self.name)
    }

    pub fn is_internal(&self) -> bool {
        matches!(self.kind(), ConnectorKind::Internal)
    }

    pub fn is_enabled(&self) -> bool {
        self.logical.is_some()
    }

    pub fn current_mode(&self) -> Option<&DisplayMode> {
        self.current_mode.and_then(|index| self.modes.get(index))
    }

    pub fn preferred_mode(&self) -> Option<&DisplayMode> {
        self.modes
            .iter()
            .find(|mode| mode.is_preferred)
            .or_else(|| self.modes.first())
    }

    /// Best available mode; used for sizing when the output is disabled.
    pub fn reference_mode(&self) -> Option<&DisplayMode> {
        self.current_mode().or_else(|| self.preferred_mode())
    }

    /// Best-effort logical width used for position math.
    ///
    /// Enabled outputs report exact logical geometry. Disabled outputs fall
    /// back to the current or preferred mode at scale 1, and finally to
    /// [`DEFAULT_LOGICAL_WIDTH`].
    pub fn reference_logical_width(&self) -> i32 {
        if let Some(logical) = &self.logical {
            return logical.width;
        }
        if let Some(mode) = self.reference_mode() {
            return mode.width as i32;
        }
        DEFAULT_LOGICAL_WIDTH
    }

    /// Short human-readable description of the current mode.
    pub fn describe_mode(&self) -> String {
        match self.current_mode() {
            Some(mode) => format!("{}x{} @ {} Hz", mode.width, mode.height, mode.refresh_hz()),
            None => "off".to_owned(),
        }
    }
}

/// DRM connector state from `/sys/class/drm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorStatus {
    pub card: String,
    pub connector: String,
    pub connected: bool,
}

impl ConnectorStatus {
    pub fn id(&self) -> OutputId {
        OutputId::new(self.connector.clone())
    }

    pub fn kind(&self) -> ConnectorKind {
        classify_connector(&self.connector)
    }
}

/// Format a scale value without a trailing `.0` for integers.
pub fn format_scale(scale: f64) -> String {
    if (scale - scale.round()).abs() < 1e-9 {
        format!("{}", scale.round() as i64)
    } else {
        format!("{scale}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_outputs(json: &str) -> Vec<DisplayOutput> {
        let map: std::collections::HashMap<String, DisplayOutput> =
            serde_json::from_str(json).expect("fixture must parse");
        let mut outputs: Vec<DisplayOutput> = map.into_values().collect();
        outputs.sort_by(|a, b| a.name.cmp(&b.name));
        outputs
    }

    #[test]
    fn classifies_connector_kinds() {
        assert_eq!(classify_connector("eDP-1"), ConnectorKind::Internal);
        assert_eq!(classify_connector("lvds-1"), ConnectorKind::Internal);
        assert_eq!(classify_connector("DSI-1"), ConnectorKind::Internal);
        assert_eq!(classify_connector("HDMI-A-1"), ConnectorKind::External);
        assert_eq!(classify_connector("DP-3"), ConnectorKind::External);
        assert_eq!(classify_connector("Unknown-1"), ConnectorKind::External);
    }

    #[test]
    fn parses_captured_single_output_fixture() {
        let outputs = parse_outputs(include_str!(
            "../../tests/fixtures/niri_outputs_single.json"
        ));
        assert_eq!(outputs.len(), 1);
        let output = &outputs[0];
        assert_eq!(output.name, "eDP-1");
        assert!(output.is_internal());
        assert!(output.is_enabled());
        assert_eq!(output.reference_logical_width(), 1920);
        assert_eq!(output.describe_mode(), "1920x1080 @ 144.001 Hz");
        assert!(output.vrr_supported);
    }

    #[test]
    fn parses_disabled_output_without_logical_geometry() {
        let outputs = parse_outputs(include_str!(
            "../../tests/fixtures/niri_outputs_external_off.json"
        ));
        let external = outputs
            .iter()
            .find(|output| output.name == "HDMI-A-1")
            .expect("fixture contains HDMI-A-1");
        assert!(!external.is_enabled());
        assert_eq!(external.describe_mode(), "off");
        // Falls back to the preferred mode width at scale 1.
        assert_eq!(external.reference_logical_width(), 3840);
    }

    #[test]
    fn formats_scale_values() {
        assert_eq!(format_scale(1.0), "1");
        assert_eq!(format_scale(2.0), "2");
        assert_eq!(format_scale(1.5), "1.5");
    }
}
