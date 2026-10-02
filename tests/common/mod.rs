//! Shared test doubles for integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use niri_display_manager::domain::display::{
    ConnectorStatus, DisplayOutput, LogicalGeometry, OutputId,
};
use niri_display_manager::domain::profile::LayoutPlan;
use niri_display_manager::infrastructure::drm_sysfs::{HardwareError, HardwareProbe};
use niri_display_manager::infrastructure::niri_ipc::{
    NiriClient, NiriError, WindowInfo, WorkspaceInfo,
};
use niri_display_manager::infrastructure::process_runner::{
    CommandSpec, ProcessError, ProcessSupervisor,
};
use niri_display_manager::service::backup_service::FileConfigStore;

#[derive(Default)]
struct NiriState {
    outputs: Vec<DisplayOutput>,
    pending_outputs: Option<Vec<DisplayOutput>>,
    windows: Vec<WindowInfo>,
    workspaces: Vec<WorkspaceInfo>,
    calls: Vec<String>,
    validate_ok: bool,
}

/// In-memory niri client.
pub struct FakeNiri {
    state: Mutex<NiriState>,
}

impl FakeNiri {
    pub fn new(outputs: Vec<DisplayOutput>) -> Self {
        Self {
            state: Mutex::new(NiriState {
                outputs,
                validate_ok: true,
                ..NiriState::default()
            }),
        }
    }

    pub fn with_pending(self, outputs: Vec<DisplayOutput>) -> Self {
        self.state.lock().unwrap().pending_outputs = Some(outputs);
        self
    }

    pub fn with_windows(self, windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Self {
        let mut state = self.state.lock().unwrap();
        state.windows = windows;
        state.workspaces = workspaces;
        drop(state);
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl NiriClient for FakeNiri {
    fn outputs(&self) -> Result<Vec<DisplayOutput>, NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("outputs".to_owned());
        Ok(state.outputs.clone())
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("windows".to_owned());
        Ok(state.windows.clone())
    }

    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("workspaces".to_owned());
        Ok(state.workspaces.clone())
    }

    fn load_config(&self) -> Result<(), NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("load_config".to_owned());
        if let Some(pending) = state.pending_outputs.take() {
            state.outputs = pending;
        }
        Ok(())
    }

    fn move_window_to_output(&self, window_id: u64, output: &OutputId) -> Result<(), NiriError> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("move_window_to_output {window_id} {output}"));
        Ok(())
    }

    fn focus_window(&self, window_id: u64) -> Result<(), NiriError> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("focus_window {window_id}"));
        Ok(())
    }

    fn focus_output(&self, output: &OutputId) -> Result<(), NiriError> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(format!("focus_output {output}"));
        Ok(())
    }

    fn enable_output(&self, output: &OutputId) -> Result<(), NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("enable_output {output}"));
        if let Some(target) = state
            .outputs
            .iter_mut()
            .find(|candidate| candidate.name == output.as_str())
        {
            let width = target.reference_logical_width();
            target.logical = Some(LogicalGeometry {
                x: 0,
                y: 0,
                width,
                height: 1080,
                scale: 1.0,
                transform: "normal".to_owned(),
            });
        }
        Ok(())
    }

    fn validate_config(&self, _path: &Path) -> Result<(), NiriError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("validate".to_owned());
        if state.validate_ok {
            Ok(())
        } else {
            Err(NiriError::CommandFailed {
                command: "niri validate".to_owned(),
                stderr: "syntax error".to_owned(),
            })
        }
    }
}

/// Connector probe with fixed data.
pub struct FakeProbe {
    pub connectors: Vec<ConnectorStatus>,
}

impl FakeProbe {
    pub fn new(connectors: Vec<ConnectorStatus>) -> Self {
        Self { connectors }
    }
}

impl HardwareProbe for FakeProbe {
    fn connectors(&self) -> Result<Vec<ConnectorStatus>, HardwareError> {
        Ok(self.connectors.clone())
    }
}

#[derive(Default)]
struct SupervisorState {
    running: bool,
    spawned: Vec<CommandSpec>,
    terminations: usize,
}

/// Supervisor that records spawns and terminations without real processes.
#[derive(Default)]
pub struct FakeSupervisor {
    state: Mutex<SupervisorState>,
}

impl FakeSupervisor {
    pub fn running() -> Self {
        Self {
            state: Mutex::new(SupervisorState {
                running: true,
                ..SupervisorState::default()
            }),
        }
    }

    pub fn spawned(&self) -> Vec<CommandSpec> {
        self.state.lock().unwrap().spawned.clone()
    }

    pub fn terminations(&self) -> usize {
        self.state.lock().unwrap().terminations
    }
}

impl ProcessSupervisor for FakeSupervisor {
    fn spawn(&self, spec: &CommandSpec) -> Result<u32, ProcessError> {
        let mut state = self.state.lock().unwrap();
        state.spawned.push(spec.clone());
        state.running = true;
        Ok(4242)
    }

    fn is_running(&self) -> Result<bool, ProcessError> {
        Ok(self.state.lock().unwrap().running)
    }

    fn terminate(&self, _grace: Duration) -> Result<(), ProcessError> {
        let mut state = self.state.lock().unwrap();
        state.terminations += 1;
        state.running = false;
        Ok(())
    }

    fn terminate_pid(&self, _pid: u32, _grace: Duration) -> Result<(), ProcessError> {
        Ok(())
    }
}

/// Build a real filesystem-backed config store inside a temporary directory.
pub fn store(root: &Path) -> FileConfigStore {
    FileConfigStore::new(
        root.join("niri/cfg/display.kdl"),
        root.join("niri/cfg/display.kdl.bak"),
        root.join("niri/config.kdl"),
    )
}

pub fn enabled_output(name: &str, width: i32) -> DisplayOutput {
    DisplayOutput {
        name: name.to_owned(),
        make: "Test".to_owned(),
        model: "Panel".to_owned(),
        serial: None,
        physical_size: Some((300, 200)),
        modes: vec![niri_display_manager::domain::display::DisplayMode {
            width: width as u32,
            height: 1080,
            refresh_rate: 60001,
            is_preferred: true,
        }],
        current_mode: Some(0),
        vrr_supported: false,
        vrr_enabled: false,
        logical: Some(LogicalGeometry {
            x: 0,
            y: 0,
            width,
            height: 1080,
            scale: 1.0,
            transform: "normal".to_owned(),
        }),
    }
}

pub fn disabled_output(name: &str, preferred_width: u32) -> DisplayOutput {
    let mut output = enabled_output(name, preferred_width as i32);
    output.logical = None;
    output.current_mode = None;
    output
}

pub fn connector(name: &str, card: &str, connected: bool) -> ConnectorStatus {
    ConnectorStatus {
        card: card.to_owned(),
        connector: name.to_owned(),
        connected,
    }
}

/// Apply planned positions/enabled flags as niri would after a reload.
pub fn realize(plan: &LayoutPlan, base: &[DisplayOutput]) -> Vec<DisplayOutput> {
    let mut result = base.to_vec();
    for expected in &plan.outputs {
        let Some(output) = result
            .iter_mut()
            .find(|candidate| candidate.name == expected.output.as_str())
        else {
            continue;
        };
        if expected.enabled {
            let default_width = output.reference_logical_width();
            let geometry = output.logical.get_or_insert_with(|| LogicalGeometry {
                x: 0,
                y: 0,
                width: default_width,
                height: 1080,
                scale: 1.0,
                transform: "normal".to_owned(),
            });
            if let Some((x, y)) = expected.position {
                geometry.x = x;
                geometry.y = y;
            }
        } else {
            output.logical = None;
        }
    }
    result
}

/// Path helper for pid files inside a temporary directory.
pub fn pid_path(root: &Path) -> PathBuf {
    root.join("mirror.pid")
}
