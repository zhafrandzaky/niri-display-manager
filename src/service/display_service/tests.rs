use super::*;
use crate::domain::display::{DisplayMode, LogicalGeometry, OutputId};
use crate::service::backup_service::{RestoreOutcome, SnapshotOutcome};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// -- Fakes ---------------------------------------------------------

struct FakeNiriState {
    outputs: Vec<DisplayOutput>,
    pending_outputs: Option<Vec<DisplayOutput>>,
    windows: Vec<WindowInfo>,
    workspaces: Vec<WorkspaceInfo>,
    calls: Vec<String>,
    validate_ok: bool,
}

struct FakeNiri {
    state: Mutex<FakeNiriState>,
}

impl FakeNiri {
    fn new(outputs: Vec<DisplayOutput>) -> Self {
        Self {
            state: Mutex::new(FakeNiriState {
                outputs,
                pending_outputs: None,
                windows: Vec::new(),
                workspaces: Vec::new(),
                calls: Vec::new(),
                validate_ok: true,
            }),
        }
    }

    fn with_pending(self, outputs: Vec<DisplayOutput>) -> Self {
        self.state.lock().unwrap().pending_outputs = Some(outputs);
        self
    }

    fn with_windows(self, windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Self {
        let mut state = self.state.lock().unwrap();
        state.windows = windows;
        state.workspaces = workspaces;
        drop(state);
        self
    }

    fn failing_validation(self) -> Self {
        self.state.lock().unwrap().validate_ok = false;
        self
    }

    fn calls(&self) -> Vec<String> {
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
        let mut state = self.state.lock().unwrap();
        state
            .calls
            .push(format!("move_window_to_output {window_id} {output}"));
        let workspace_id = state
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .and_then(|window| window.workspace_id);
        if let Some(workspace_id) = workspace_id
            && let Some(workspace) = state
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == workspace_id)
        {
            workspace.output = Some(output.as_str().to_owned());
        }
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
                stderr: "syntax error at line 4".to_owned(),
            })
        }
    }
}

struct FakeProbe {
    connectors: Vec<ConnectorStatus>,
}

impl FakeProbe {
    fn new(connectors: Vec<ConnectorStatus>) -> Self {
        Self { connectors }
    }
}

impl HardwareProbe for FakeProbe {
    fn connectors(&self) -> Result<Vec<ConnectorStatus>, HardwareError> {
        Ok(self.connectors.clone())
    }
}

struct FakeConfigState {
    display: String,
    backup: Option<String>,
    registered: bool,
    mode: ConfigMode,
}

struct FakeConfig {
    state: Mutex<FakeConfigState>,
}

impl FakeConfig {
    fn with_display(display: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(FakeConfigState {
                display: display.into(),
                backup: None,
                registered: true,
                mode: ConfigMode::Modular,
            }),
        }
    }

    fn with_backup(display: impl Into<String>, backup: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(FakeConfigState {
                display: display.into(),
                backup: Some(backup.into()),
                registered: true,
                mode: ConfigMode::Modular,
            }),
        }
    }

    fn unregistered(self) -> Self {
        self.state.lock().unwrap().registered = false;
        self
    }

    fn display(&self) -> String {
        self.state.lock().unwrap().display.clone()
    }

    fn backup(&self) -> Option<String> {
        self.state.lock().unwrap().backup.clone()
    }
}

impl ConfigStore for FakeConfig {
    fn mode(&self) -> ConfigMode {
        self.state.lock().unwrap().mode
    }

    fn main_config_path(&self) -> &Path {
        Path::new("/fake/config.kdl")
    }

    fn managed_config_path(&self) -> &Path {
        Path::new("/fake/display.kdl")
    }

    fn backup_path(&self) -> &Path {
        Path::new("/fake/display.kdl.bak")
    }

    fn read_managed_config(&self) -> Result<String, ConfigError> {
        Ok(self.state.lock().unwrap().display.clone())
    }

    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError> {
        let mut state = self.state.lock().unwrap();
        if state.backup.is_some() {
            return Ok(SnapshotOutcome::AlreadyExists);
        }
        state.backup = Some(state.display.clone());
        Ok(SnapshotOutcome::Created)
    }

    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError> {
        let mut state = self.state.lock().unwrap();
        state.display = kdl_parser::splice(&state.display, section);
        Ok(())
    }

    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError> {
        let mut state = self.state.lock().unwrap();
        if let Some(backup) = state.backup.clone() {
            state.display = backup;
            return Ok(RestoreOutcome::RestoredFromBackup);
        }
        let (updated, removed) = kdl_parser::remove_section(&state.display);
        if removed {
            state.display = updated;
            return Ok(RestoreOutcome::RemovedManagedSection);
        }
        Ok(RestoreOutcome::NoChange)
    }

    fn display_file_registered(&self) -> Result<bool, ConfigError> {
        Ok(self.state.lock().unwrap().registered)
    }
}

#[derive(Default)]
struct FakeSupervisorState {
    running: bool,
    spawned: Vec<CommandSpec>,
    terminations: usize,
}

#[derive(Default)]
struct FakeSupervisor {
    state: Mutex<FakeSupervisorState>,
}

impl FakeSupervisor {
    fn running() -> Self {
        Self {
            state: Mutex::new(FakeSupervisorState {
                running: true,
                ..FakeSupervisorState::default()
            }),
        }
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

// -- Helpers -------------------------------------------------------

fn enabled_output(name: &str, width: i32) -> DisplayOutput {
    DisplayOutput {
        name: name.to_owned(),
        make: "Test".to_owned(),
        model: "Panel".to_owned(),
        serial: None,
        physical_size: Some((300, 200)),
        modes: vec![DisplayMode {
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

fn disabled_output(name: &str, preferred_width: u32) -> DisplayOutput {
    let mut output = enabled_output(name, preferred_width as i32);
    output.logical = None;
    output.current_mode = None;
    output
}

/// Apply the planned positions/enabled flags to a set of outputs, as niri
/// would after a successful reload.
fn realize(plan: &LayoutPlan, base: &[DisplayOutput]) -> Vec<DisplayOutput> {
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

fn pid_file(directory: &tempfile::TempDir) -> MirrorPidFile {
    MirrorPidFile::new(directory.path().join("mirror.pid"))
}

struct Fixture {
    service: DisplayService<ArcedNiri, FakeProbe, ArcedConfig, ArcedSupervisor>,
    niri: std::sync::Arc<FakeNiri>,
    config: std::sync::Arc<FakeConfig>,
    supervisor: std::sync::Arc<FakeSupervisor>,
    pid_path: PathBuf,
    _directory: tempfile::TempDir,
}

// The service owns its adapters by value; these Arc wrappers keep handles
// available to tests while satisfying the trait bounds.

struct ArcedNiri(std::sync::Arc<FakeNiri>);
struct ArcedConfig(std::sync::Arc<FakeConfig>);
struct ArcedSupervisor(std::sync::Arc<FakeSupervisor>);

impl NiriClient for ArcedNiri {
    fn outputs(&self) -> Result<Vec<DisplayOutput>, NiriError> {
        self.0.outputs()
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, NiriError> {
        self.0.windows()
    }
    fn workspaces(&self) -> Result<Vec<WorkspaceInfo>, NiriError> {
        self.0.workspaces()
    }
    fn load_config(&self) -> Result<(), NiriError> {
        self.0.load_config()
    }
    fn move_window_to_output(&self, window_id: u64, output: &OutputId) -> Result<(), NiriError> {
        self.0.move_window_to_output(window_id, output)
    }
    fn focus_window(&self, window_id: u64) -> Result<(), NiriError> {
        self.0.focus_window(window_id)
    }
    fn focus_output(&self, output: &OutputId) -> Result<(), NiriError> {
        self.0.focus_output(output)
    }
    fn enable_output(&self, output: &OutputId) -> Result<(), NiriError> {
        self.0.enable_output(output)
    }
    fn validate_config(&self, path: &Path) -> Result<(), NiriError> {
        self.0.validate_config(path)
    }
}

impl ConfigStore for ArcedConfig {
    fn mode(&self) -> ConfigMode {
        self.0.mode()
    }
    fn main_config_path(&self) -> &Path {
        self.0.main_config_path()
    }
    fn managed_config_path(&self) -> &Path {
        self.0.managed_config_path()
    }
    fn backup_path(&self) -> &Path {
        self.0.backup_path()
    }
    fn read_managed_config(&self) -> Result<String, ConfigError> {
        self.0.read_managed_config()
    }
    fn snapshot(&self) -> Result<SnapshotOutcome, ConfigError> {
        self.0.snapshot()
    }
    fn write_managed_section(&self, section: &str) -> Result<(), ConfigError> {
        self.0.write_managed_section(section)
    }
    fn restore_snapshot(&self) -> Result<RestoreOutcome, ConfigError> {
        self.0.restore_snapshot()
    }
    fn display_file_registered(&self) -> Result<bool, ConfigError> {
        self.0.display_file_registered()
    }
}

impl ProcessSupervisor for ArcedSupervisor {
    fn spawn(&self, spec: &CommandSpec) -> Result<u32, ProcessError> {
        self.0.spawn(spec)
    }
    fn is_running(&self) -> Result<bool, ProcessError> {
        self.0.is_running()
    }
    fn terminate(&self, grace: Duration) -> Result<(), ProcessError> {
        self.0.terminate(grace)
    }
    fn terminate_pid(&self, pid: u32, grace: Duration) -> Result<(), ProcessError> {
        self.0.terminate_pid(pid, grace)
    }
}

fn make_service(
    niri: FakeNiri,
    connectors: Vec<ConnectorStatus>,
    config: FakeConfig,
    supervisor: FakeSupervisor,
) -> Fixture {
    let niri = std::sync::Arc::new(niri);
    let config = std::sync::Arc::new(config);
    let supervisor = std::sync::Arc::new(supervisor);
    let directory = tempfile::tempdir().unwrap();
    let pid_path = directory.path().join("mirror.pid");
    let service = DisplayService::new(
        ArcedNiri(niri.clone()),
        FakeProbe::new(connectors),
        ArcedConfig(config.clone()),
        ArcedSupervisor(supervisor.clone()),
        pid_file(&directory),
    )
    .with_verification(3, Duration::from_millis(1));
    Fixture {
        service,
        niri,
        config,
        supervisor,
        pid_path,
        _directory: directory,
    }
}

fn mirror_window() -> WindowInfo {
    WindowInfo {
        id: 7,
        title: Some("Wayland Output Mirror for eDP-1".to_owned()),
        app_id: Some("at.yrlf.wl_mirror".to_owned()),
        pid: Some(4242),
        workspace_id: Some(1),
        is_focused: false,
    }
}

fn mirror_workspace() -> WorkspaceInfo {
    WorkspaceInfo {
        id: 1,
        name: None,
        output: Some("HDMI-A-1".to_owned()),
        is_active: true,
    }
}

// -- Tests ---------------------------------------------------------

#[test]
fn applies_extend_right_with_validation_reload_and_verification() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let plan = profile::plan_profile(ProfileKind::ExtendRight, &base, &[]).unwrap();
    let pending = realize(&plan, &base);
    let fixture = make_service(
        FakeNiri::new(base).with_pending(pending),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let report = fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap();
    assert_eq!(report.profile, ProfileKind::ExtendRight);
    assert!(report.warnings.is_empty());
    assert_eq!(report.layout.outputs[1].position, Some((1920, 0)));

    let calls = fixture.niri.calls();
    assert_eq!(calls[0], "outputs");
    assert!(calls.contains(&"validate".to_owned()));
    assert!(calls.contains(&"load_config".to_owned()));

    let display = fixture.config.display();
    assert!(display.starts_with("// user config\n"));
    assert!(display.contains("// profile: extend-right"));
    assert!(display.contains("position x=1920 y=0"));
    assert_eq!(fixture.config.backup().as_deref(), Some("// user config\n"));
}

#[test]
fn rolls_back_when_validation_fails() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let fixture = make_service(
        FakeNiri::new(base).failing_validation(),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let error = fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap_err();
    assert!(matches!(error, ServiceError::ValidationFailed(_)));
    assert_eq!(fixture.config.display(), "// user config\n");
    let calls = fixture.niri.calls();
    assert!(calls.contains(&"validate".to_owned()));
    assert!(calls.contains(&"load_config".to_owned()));
}

#[test]
fn rolls_back_when_the_layout_does_not_match() {
    // The fake niri keeps the external display at x=0 after reload, so the
    // expected position (1920, 0) is never observed.
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let fixture = make_service(
        FakeNiri::new(base),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let error = fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap_err();
    assert!(
        matches!(error, ServiceError::VerificationFailed(_)),
        "got {error}"
    );
    assert_eq!(fixture.config.display(), "// user config\n");
}

#[test]
fn internal_only_restores_backup_and_reenables_the_panel() {
    let original = "// user config\n";
    let section = kdl_parser::render_section(
        &profile::plan_profile(
            ProfileKind::ExtendRight,
            &[
                enabled_output("eDP-1", 1920),
                enabled_output("HDMI-A-1", 2560),
            ],
            &[],
        )
        .unwrap(),
    );
    let current = kdl_parser::splice(original, &section);
    let fixture = make_service(
        FakeNiri::new(vec![
            disabled_output("eDP-1", 1920),
            enabled_output("HDMI-A-1", 2560),
        ]),
        vec![],
        FakeConfig::with_backup(current, original),
        FakeSupervisor::default(),
    );

    fixture
        .service
        .apply_profile(ProfileKind::InternalOnly)
        .unwrap();
    assert_eq!(fixture.config.display(), original);
    let calls = fixture.niri.calls();
    assert!(
        calls.iter().any(|call| call == "enable_output eDP-1"),
        "expected the panel to be re-enabled: {calls:?}"
    );
}

#[test]
fn mirror_apply_spawns_wl_mirror_targeting_the_external_display() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let layout = profile::plan_profile(ProfileKind::Mirror, &base, &[]).unwrap();
    let pending = realize(&layout, &base);
    let fixture = make_service(
        FakeNiri::new(base)
            .with_pending(pending)
            .with_windows(vec![mirror_window()], vec![mirror_workspace()]),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let report = fixture.service.apply_profile(ProfileKind::Mirror).unwrap();
    assert_eq!(report.profile, ProfileKind::Mirror);

    let supervisor = fixture.supervisor.state.lock().unwrap();
    assert_eq!(supervisor.spawned.len(), 1);
    assert_eq!(supervisor.spawned[0].program, "wl-mirror");
    assert_eq!(
        supervisor.spawned[0].args,
        vec!["--fullscreen-output", "HDMI-A-1", "eDP-1"]
    );
    drop(supervisor);

    let pid_file = MirrorPidFile::new(&fixture.pid_path);
    assert_eq!(pid_file.read().unwrap(), Some(4242));
}

#[test]
fn mirror_apply_fails_and_stops_the_child_when_no_window_appears() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let layout = profile::plan_profile(ProfileKind::Mirror, &base, &[]).unwrap();
    let pending = realize(&layout, &base);
    let fixture = make_service(
        FakeNiri::new(base).with_pending(pending),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let error = fixture
        .service
        .apply_profile(ProfileKind::Mirror)
        .unwrap_err();
    assert!(
        matches!(error, ServiceError::MirrorFailed { .. }),
        "got {error}"
    );
    assert_eq!(fixture.supervisor.state.lock().unwrap().terminations, 1);
    let pid_file = MirrorPidFile::new(&fixture.pid_path);
    assert_eq!(pid_file.read().unwrap(), None);
    // A failed mirror start rolls the layout back like any other failure.
    assert_eq!(fixture.config.display(), "// user config\n");
}

#[test]
fn mirror_window_matching_accepts_pid_and_app_id_spellings() {
    let real = WindowInfo {
        id: 1,
        title: None,
        app_id: Some("at.yrlf.wl_mirror".to_owned()),
        pid: Some(100),
        workspace_id: None,
        is_focused: false,
    };
    // Exact child pid wins even if the app id were unknown.
    assert!(is_mirror_window(&real, 100));
    // Current wl-mirror app id matches without a pid.
    assert!(is_mirror_window(&real, 999));

    let legacy = WindowInfo {
        app_id: Some("wl-mirror".to_owned()),
        pid: None,
        ..real.clone()
    };
    assert!(is_mirror_window(&legacy, 999));

    let unrelated = WindowInfo {
        app_id: Some("firefox".to_owned()),
        pid: None,
        ..real
    };
    assert!(!is_mirror_window(&unrelated, 999));
}

#[test]
fn mirror_window_is_moved_to_the_target_output() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let layout = profile::plan_profile(ProfileKind::Mirror, &base, &[]).unwrap();
    let pending = realize(&layout, &base);
    let fixture = make_service(
        FakeNiri::new(base).with_pending(pending).with_windows(
            vec![mirror_window()],
            vec![WorkspaceInfo {
                id: 1,
                name: None,
                output: Some("eDP-1".to_owned()),
                is_active: true,
            }],
        ),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    fixture.service.apply_profile(ProfileKind::Mirror).unwrap();
    let calls = fixture.niri.calls();
    assert!(
        calls
            .iter()
            .any(|call| call == "move_window_to_output 7 HDMI-A-1"),
        "expected the window to be moved: {calls:?}"
    );
    assert!(calls.iter().any(|call| call == "focus_window 7"));
}

#[test]
fn switching_away_from_mirror_terminates_the_child() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let plan = profile::plan_profile(ProfileKind::ExtendRight, &base, &[]).unwrap();
    let pending = realize(&plan, &base);
    let fixture = make_service(
        FakeNiri::new(base).with_pending(pending),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::running(),
    );
    MirrorPidFile::new(&fixture.pid_path).write(4242).unwrap();

    fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap();
    assert_eq!(fixture.supervisor.state.lock().unwrap().terminations, 1);
    assert_eq!(MirrorPidFile::new(&fixture.pid_path).read().unwrap(), None);
}

#[test]
fn planning_failure_leaves_configuration_untouched() {
    let fixture = make_service(
        FakeNiri::new(vec![enabled_output("eDP-1", 1920)]),
        vec![],
        FakeConfig::with_display("// user config\n"),
        FakeSupervisor::default(),
    );

    let error = fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap_err();
    assert!(
        matches!(error, ServiceError::Plan(PlanError::NoExternalPort)),
        "got {error}"
    );
    assert_eq!(fixture.config.display(), "// user config\n");
    assert_eq!(fixture.config.backup(), None);
}

#[test]
fn refresh_state_reports_profile_include_and_mirror_status() {
    let section = kdl_parser::render_section(
        &profile::plan_profile(
            ProfileKind::Mirror,
            &[
                enabled_output("eDP-1", 1920),
                enabled_output("HDMI-A-1", 2560),
            ],
            &[],
        )
        .unwrap(),
    );
    let fixture = make_service(
        FakeNiri::new(vec![enabled_output("eDP-1", 1920)]),
        vec![],
        FakeConfig::with_display(kdl_parser::splice("", &section)),
        FakeSupervisor::running(),
    );

    let state = fixture.service.refresh_state().unwrap();
    assert_eq!(state.active_profile, Some(ProfileKind::Mirror));
    assert!(state.mirror_running);
    assert_eq!(state.config_mode, ConfigMode::Modular);
    assert!(state.display_file_registered);
    assert_eq!(state.outputs.len(), 1);
}

#[test]
fn setup_required_state_fails_fast_for_writes_but_allows_reset() {
    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let fixture = make_service(
        FakeNiri::new(base),
        vec![],
        FakeConfig::with_display("// user config\n").unregistered(),
        FakeSupervisor::default(),
    );

    let error = fixture
        .service
        .apply_profile(ProfileKind::ExtendRight)
        .unwrap_err();
    assert!(
        matches!(error, ServiceError::SetupRequired(_)),
        "got {error}"
    );
    assert_eq!(fixture.config.display(), "// user config\n");
    assert_eq!(fixture.config.backup(), None);
    let calls = fixture.niri.calls();
    assert!(!calls.contains(&"validate".to_owned()));

    // The reset profile still works: it only restores, it never writes.
    fixture
        .service
        .apply_profile(ProfileKind::InternalOnly)
        .unwrap();
}

#[test]
fn initialize_cleans_up_stale_pid_files() {
    let fixture = make_service(
        FakeNiri::new(vec![]),
        vec![],
        FakeConfig::with_display(""),
        FakeSupervisor::default(),
    );
    MirrorPidFile::new(&fixture.pid_path)
        .write(99_999_999)
        .unwrap();
    fixture.service.initialize().unwrap();
    assert_eq!(MirrorPidFile::new(&fixture.pid_path).read().unwrap(), None);
}
