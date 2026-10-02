//! Application service orchestrating profile application.
//!
//! Every apply follows the same fail-safe pipeline:
//!
//! 1. Probe niri outputs and DRM connectors.
//! 2. Plan the layout (pure domain logic); refuse unsafe profiles early.
//! 3. Stop any running mirror child.
//! 4. Snapshot the pristine configuration once.
//! 5. Write the managed section atomically.
//! 6. Validate with `niri validate`; reload with `niri msg action
//!    load-config-file`; verify the observed layout.
//! 7. Roll back to the snapshot on any failure.
//!
//! All external effects go through the injected [`NiriClient`],
//! [`HardwareProbe`], [`ConfigStore`], and [`ProcessSupervisor`] traits.

use std::time::Duration;

use super::backup_service::{ConfigError, ConfigStore};
use crate::domain::display::{ConnectorStatus, DisplayOutput};
use crate::domain::profile::{self, LayoutPlan, MirrorPlan, PlanError, ProfileKind};
use crate::infrastructure::drm_sysfs::{HardwareError, HardwareProbe};
use crate::infrastructure::kdl_parser;
use crate::infrastructure::niri_ipc::{NiriClient, NiriError, WindowInfo, WorkspaceInfo};
use crate::infrastructure::process_runner::{
    CommandSpec, MirrorPidFile, ProcessError, ProcessSupervisor, process_is_wl_mirror,
};

/// Errors surfaced by the display service.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("niri IPC error: {0}")]
    Niri(#[from] NiriError),
    #[error("hardware probe error: {0}")]
    Hardware(#[from] HardwareError),
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),
    #[error("process error: {0}")]
    Process(#[from] ProcessError),
    #[error("cannot plan profile: {0}")]
    Plan(#[from] PlanError),
    #[error(
        "niri rejected the generated configuration; the previous configuration was restored: {0}"
    )]
    ValidationFailed(String),
    #[error(
        "the display layout did not match the requested profile; the previous configuration was restored: {0}"
    )]
    VerificationFailed(String),
    #[error("the mirror window did not appear on {target}: {detail}")]
    MirrorFailed { target: String, detail: String },
}

/// Observed system state for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemState {
    pub outputs: Vec<DisplayOutput>,
    pub connectors: Vec<ConnectorStatus>,
    pub active_profile: Option<ProfileKind>,
    pub mirror_running: bool,
    pub include_registered: bool,
}

/// Outcome of a successful profile application.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyReport {
    pub profile: ProfileKind,
    pub warnings: Vec<String>,
    pub layout: LayoutPlan,
}

/// Polling policy for post-reload verification.
#[derive(Debug, Clone, Copy)]
pub struct VerificationPolicy {
    pub attempts: u32,
    pub interval: Duration,
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self {
            attempts: 30,
            interval: Duration::from_millis(100),
        }
    }
}

/// Grace period before a mirror child is force-killed.
const MIRROR_TERMINATION_GRACE: Duration = Duration::from_secs(2);

/// Orchestrates display profiles through injected adapters.
pub struct DisplayService<N, H, S, P>
where
    N: NiriClient,
    H: HardwareProbe,
    S: ConfigStore,
    P: ProcessSupervisor,
{
    niri: N,
    probe: H,
    config: S,
    supervisor: P,
    pid_file: MirrorPidFile,
    verification: VerificationPolicy,
}

impl<N, H, S, P> DisplayService<N, H, S, P>
where
    N: NiriClient,
    H: HardwareProbe,
    S: ConfigStore,
    P: ProcessSupervisor,
{
    pub fn new(niri: N, probe: H, config: S, supervisor: P, pid_file: MirrorPidFile) -> Self {
        Self {
            niri,
            probe,
            config,
            supervisor,
            pid_file,
            verification: VerificationPolicy::default(),
        }
    }

    /// Override the verification polling policy (used by tests).
    pub fn with_verification(mut self, attempts: u32, interval: Duration) -> Self {
        self.verification = VerificationPolicy { attempts, interval };
        self
    }

    /// Startup hook: clean up a wl-mirror orphaned by a previous crash.
    pub fn initialize(&self) -> Result<(), ServiceError> {
        if self.pid_file.cleanup_orphan(process_is_wl_mirror)? {
            log::warn!("terminated an orphaned wl-mirror process from a previous session");
        }
        Ok(())
    }

    /// Observe the current system state for the UI.
    pub fn refresh_state(&self) -> Result<SystemState, ServiceError> {
        let outputs = self.niri.outputs()?;
        let connectors = self.probe.connectors()?;
        let active_profile = kdl_parser::section_profile(&self.config.read_display_config()?);
        let mirror_running = self.supervisor.is_running()?;
        let include_registered = self.config.is_registered_in_main_config()?;
        Ok(SystemState {
            outputs,
            connectors,
            active_profile,
            mirror_running,
            include_registered,
        })
    }

    /// Apply a profile using the fail-safe pipeline.
    pub fn apply_profile(&self, profile: ProfileKind) -> Result<ApplyReport, ServiceError> {
        let outputs = self.niri.outputs()?;
        let connectors = self.probe.connectors()?;

        let layout = profile::plan_profile(profile, &outputs, &connectors)?;
        let mut warnings = collect_warnings(profile, &layout, &outputs, &connectors);
        if !self.config.is_registered_in_main_config()? {
            warnings.push(
                "display.kdl is not included by the main niri configuration; changes may not take effect"
                    .to_owned(),
            );
        }

        // Never leave a stale mirror running when the profile changes.
        self.stop_mirror()?;

        self.config.snapshot()?;

        if profile == ProfileKind::InternalOnly {
            let _outcome = self.config.restore_snapshot()?;
        } else {
            let section = kdl_parser::render_section(&layout);
            self.config.write_managed_section(&section)?;
        }

        if let Err(error) = self.niri.validate_config(self.config.main_config_path()) {
            self.rollback();
            return Err(ServiceError::ValidationFailed(error.to_string()));
        }
        if let Err(error) = self.niri.load_config() {
            self.rollback();
            return Err(ServiceError::Niri(error));
        }
        if let Err(mismatch) = self.verify_layout(&layout) {
            self.rollback();
            return Err(ServiceError::VerificationFailed(mismatch));
        }

        if profile == ProfileKind::InternalOnly {
            self.ensure_internal_enabled(&outputs)?;
        }
        if let Some(focus) = &layout.focus_output
            && let Err(error) = self.niri.focus_output(focus)
        {
            log::warn!("failed to focus output {focus}: {error}");
        }
        if profile == ProfileKind::Mirror {
            let mirror_plan = profile::plan_mirror(&outputs)?;
            self.start_mirror(&mirror_plan)?;
        }

        Ok(ApplyReport {
            profile,
            warnings,
            layout,
        })
    }

    /// Stop the supervised mirror process and clear its pid file.
    pub fn stop_mirror(&self) -> Result<(), ServiceError> {
        if self.pid_file.read()?.is_some() || self.supervisor.is_running()? {
            self.supervisor.terminate(MIRROR_TERMINATION_GRACE)?;
        }
        self.pid_file.remove()?;
        Ok(())
    }

    /// Shutdown hook used when the application exits.
    pub fn shutdown(&self) -> Result<(), ServiceError> {
        self.stop_mirror()
    }

    fn rollback(&self) {
        if let Err(error) = self.config.restore_snapshot() {
            log::error!("rollback failed: {error}");
        }
        if let Err(error) = self.niri.load_config() {
            log::error!("rollback reload failed: {error}");
        }
    }

    fn verify_layout(&self, layout: &LayoutPlan) -> Result<(), String> {
        let mut last_mismatch = String::from("no outputs observed");
        let attempts = self.verification.attempts.max(1);
        for attempt in 0..attempts {
            let outputs = self.niri.outputs().map_err(|error| error.to_string())?;
            match profile::verify_plan(layout, &outputs) {
                Ok(()) => return Ok(()),
                Err(mismatch) => last_mismatch = mismatch,
            }
            if attempt + 1 < attempts {
                std::thread::sleep(self.verification.interval);
            }
        }
        Err(last_mismatch)
    }

    fn ensure_internal_enabled(&self, outputs: &[DisplayOutput]) -> Result<(), ServiceError> {
        if let Some(internal) = outputs.iter().find(|output| output.is_internal())
            && !internal.is_enabled()
        {
            self.niri.enable_output(&internal.id())?;
        }
        Ok(())
    }

    fn start_mirror(&self, plan: &MirrorPlan) -> Result<(), ServiceError> {
        let spec = CommandSpec::new(MirrorPlan::program(), plan.command_args());
        let pid = self.supervisor.spawn(&spec)?;
        if let Err(error) = self.pid_file.write(pid) {
            let _ = self.supervisor.terminate(MIRROR_TERMINATION_GRACE);
            return Err(ServiceError::Process(error));
        }
        if let Err(error) = self.verify_mirror_window(plan) {
            let _ = self.supervisor.terminate(MIRROR_TERMINATION_GRACE);
            let _ = self.pid_file.remove();
            return Err(error);
        }
        Ok(())
    }

    fn verify_mirror_window(&self, plan: &MirrorPlan) -> Result<(), ServiceError> {
        let mut last_detail = String::from("mirror window did not appear");
        let attempts = self.verification.attempts.max(1);
        for attempt in 0..attempts {
            if !self.supervisor.is_running()? {
                return Err(ServiceError::MirrorFailed {
                    target: plan.target.to_string(),
                    detail: "wl-mirror exited immediately (check wl-mirror compositor support)"
                        .to_owned(),
                });
            }
            let windows = self.niri.windows()?;
            let workspaces = self.niri.workspaces()?;
            if let Some(window) = find_mirror_window(&windows) {
                match window_output(window, &workspaces).as_deref() {
                    Some(output) if output == plan.target.as_str() => return Ok(()),
                    Some(output) => {
                        last_detail =
                            format!("mirror window is on {output} instead of {}", plan.target);
                        let _ = self.niri.move_window_to_output(window.id, &plan.target);
                        let _ = self.niri.focus_window(window.id);
                    }
                    None => {
                        last_detail = "mirror window has no output yet".to_owned();
                    }
                }
            }
            if attempt + 1 < attempts {
                std::thread::sleep(self.verification.interval);
            }
        }
        Err(ServiceError::MirrorFailed {
            target: plan.target.to_string(),
            detail: last_detail,
        })
    }
}

fn find_mirror_window(windows: &[WindowInfo]) -> Option<&WindowInfo> {
    windows.iter().find(|window| {
        window
            .app_id
            .as_deref()
            .is_some_and(|app_id| app_id.contains("wl-mirror"))
    })
}

fn window_output(window: &WindowInfo, workspaces: &[WorkspaceInfo]) -> Option<String> {
    let workspace_id = window.workspace_id?;
    workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .and_then(|workspace| workspace.output.clone())
}

fn collect_warnings(
    profile: ProfileKind,
    layout: &LayoutPlan,
    outputs: &[DisplayOutput],
    connectors: &[ConnectorStatus],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for planned in &layout.outputs {
        if planned.enabled && !planned.expect_present {
            let port = planned.output.as_str();
            let connected = connectors
                .iter()
                .any(|connector| connector.connector == port && connector.connected);
            if connected {
                warnings.push(format!(
                    "{port} is connected but not active in niri yet; the configuration will apply once niri adopts it"
                ));
            } else {
                warnings.push(format!(
                    "{port} has no cable attached; the configuration is saved and will apply when a display is connected"
                ));
            }
        }
    }
    if matches!(profile, ProfileKind::ExtendRight | ProfileKind::ExtendLeft)
        && let Some(external) = outputs.iter().find(|output| !output.is_internal())
        && !external.is_enabled()
    {
        warnings.push(format!(
            "{} is present but currently off; the profile will enable it",
            external.name
        ));
    }
    warnings
}

#[cfg(test)]
mod tests;
