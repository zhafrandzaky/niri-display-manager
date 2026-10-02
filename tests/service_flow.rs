//! Integration tests for the service flow against the real file store.
//!
//! These tests exercise the full apply pipeline with in-memory niri and
//! supervisor doubles but a real `FileConfigStore`, verifying the on-disk
//! results: managed sections, pristine backups, and rollback behavior.

mod common;

use std::fs;
use std::time::Duration;

use common::{
    FakeNiri, FakeProbe, FakeSupervisor, connector, disabled_output, enabled_output, pid_path,
    realize, store,
};
use niri_display_manager::domain::profile::{self, PlanError, ProfileKind};
use niri_display_manager::infrastructure::kdl_parser;
use niri_display_manager::infrastructure::niri_ipc::{WindowInfo, WorkspaceInfo};
use niri_display_manager::infrastructure::process_runner::MirrorPidFile;
use niri_display_manager::service::backup_service::{ConfigStore, FileConfigStore};
use niri_display_manager::service::display_service::{DisplayService, ServiceError};

type Service = DisplayService<FakeNiri, FakeProbe, FileConfigStore, FakeSupervisor>;

fn make_service(
    niri: FakeNiri,
    connectors: Vec<niri_display_manager::domain::display::ConnectorStatus>,
    config: FileConfigStore,
    supervisor: FakeSupervisor,
    directory: &tempfile::TempDir,
) -> Service {
    DisplayService::new(
        niri,
        FakeProbe::new(connectors),
        config,
        supervisor,
        MirrorPidFile::new(pid_path(directory.path())),
    )
    .with_verification(3, Duration::from_millis(1))
}

fn prepare_store(directory: &tempfile::TempDir, display: &str) -> FileConfigStore {
    let config = store(directory.path());
    fs::create_dir_all(config.display_config_path().parent().unwrap()).unwrap();
    fs::write(config.display_config_path(), display).unwrap();
    fs::write(config.main_config_path(), "include \"./cfg/display.kdl\"\n").unwrap();
    config
}

#[test]
fn extend_right_writes_managed_section_and_pristine_backup() {
    let directory = tempfile::tempdir().unwrap();
    let config = prepare_store(&directory, "// user config\n");

    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let plan = profile::plan_profile(ProfileKind::ExtendRight, &base, &[]).unwrap();
    let pending = realize(&plan, &base);

    let service = make_service(
        FakeNiri::new(base).with_pending(pending),
        vec![],
        config.clone(),
        FakeSupervisor::default(),
        &directory,
    );

    let report = service.apply_profile(ProfileKind::ExtendRight).unwrap();
    assert_eq!(report.profile, ProfileKind::ExtendRight);

    let written = fs::read_to_string(config.display_config_path()).unwrap();
    assert!(written.starts_with("// user config\n"));
    assert!(written.contains("// profile: extend-right"));
    assert!(written.contains("output \"HDMI-A-1\" {"));
    assert!(written.contains("position x=1920 y=0"));

    let backup = fs::read_to_string(directory.path().join("niri/cfg/display.kdl.bak")).unwrap();
    assert_eq!(backup, "// user config\n");
}

#[test]
fn internal_only_reset_restores_the_pristine_file() {
    let directory = tempfile::tempdir().unwrap();
    let original = "// user config\noutput \"DP-1\" {\n    scale 2\n}\n";
    let config = prepare_store(&directory, original);

    // Simulate a previous extend: pristine snapshot plus managed section.
    config.snapshot().unwrap();
    let plan = profile::plan_profile(
        ProfileKind::ExtendRight,
        &[
            enabled_output("eDP-1", 1920),
            enabled_output("HDMI-A-1", 2560),
        ],
        &[],
    )
    .unwrap();
    config
        .write_managed_section(&kdl_parser::render_section(&plan))
        .unwrap();
    assert!(
        config
            .read_display_config()
            .unwrap()
            .contains("managed section")
    );

    let service = make_service(
        FakeNiri::new(vec![
            disabled_output("eDP-1", 1920),
            enabled_output("HDMI-A-1", 2560),
        ]),
        vec![],
        config.clone(),
        FakeSupervisor::default(),
        &directory,
    );

    service.apply_profile(ProfileKind::InternalOnly).unwrap();
    assert_eq!(
        fs::read_to_string(config.display_config_path()).unwrap(),
        original
    );
}

#[test]
fn extend_without_a_cable_persists_and_does_not_fail_verification() {
    let directory = tempfile::tempdir().unwrap();
    let config = prepare_store(&directory, "// user config\n");

    let service = make_service(
        // niri only knows about the internal panel; the HDMI port exists in
        // sysfs but has no cable attached.
        FakeNiri::new(vec![enabled_output("eDP-1", 1920)]),
        vec![connector("HDMI-A-1", "card1", false)],
        config.clone(),
        FakeSupervisor::default(),
        &directory,
    );

    let report = service.apply_profile(ProfileKind::ExtendRight).unwrap();
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("no cable attached"));

    let written = fs::read_to_string(config.display_config_path()).unwrap();
    assert!(written.contains("output \"HDMI-A-1\" {"));
    assert!(written.contains("position x=1920 y=0"));
}

#[test]
fn external_only_is_refused_without_a_connected_display() {
    let directory = tempfile::tempdir().unwrap();
    let config = prepare_store(&directory, "// user config\n");

    let service = make_service(
        FakeNiri::new(vec![enabled_output("eDP-1", 1920)]),
        vec![connector("HDMI-A-1", "card1", false)],
        config.clone(),
        FakeSupervisor::default(),
        &directory,
    );

    let error = service
        .apply_profile(ProfileKind::ExternalOnly)
        .unwrap_err();
    assert!(matches!(
        error,
        ServiceError::Plan(PlanError::ExternalNotConnected)
    ));
    assert_eq!(
        fs::read_to_string(config.display_config_path()).unwrap(),
        "// user config\n"
    );
    assert!(!directory.path().join("niri/cfg/display.kdl.bak").exists());
}

#[test]
fn mirror_flow_spawns_wl_mirror_and_records_the_pid() {
    let directory = tempfile::tempdir().unwrap();
    let config = prepare_store(&directory, "// user config\n");

    let base = vec![
        enabled_output("eDP-1", 1920),
        enabled_output("HDMI-A-1", 2560),
    ];
    let plan = profile::plan_profile(ProfileKind::Mirror, &base, &[]).unwrap();
    let pending = realize(&plan, &base);

    let service = make_service(
        FakeNiri::new(base).with_pending(pending).with_windows(
            vec![WindowInfo {
                id: 7,
                title: Some("Wayland Mirror".to_owned()),
                app_id: Some("wl-mirror".to_owned()),
                workspace_id: Some(1),
                is_focused: false,
            }],
            vec![WorkspaceInfo {
                id: 1,
                name: None,
                output: Some("HDMI-A-1".to_owned()),
                is_active: true,
            }],
        ),
        vec![],
        config.clone(),
        FakeSupervisor::default(),
        &directory,
    );

    service.apply_profile(ProfileKind::Mirror).unwrap();
    let pid_file = MirrorPidFile::new(pid_path(directory.path()));
    assert_eq!(pid_file.read().unwrap(), Some(4242));

    service.stop_mirror().unwrap();
    assert_eq!(pid_file.read().unwrap(), None);
}
