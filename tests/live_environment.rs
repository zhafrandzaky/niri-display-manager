//! Live environment tests against the real niri session and DRM sysfs.
//!
//! These tests are `#[ignore]` by default because they require a running niri
//! session. Run them with:
//!
//! ```text
//! cargo test -- --ignored
//! ```
//!
//! They are strictly read-only with respect to the user configuration: the
//! generated KDL is written to a temporary directory and checked with the real
//! `niri validate` command. No live compositor state is modified.

use std::sync::Arc;
use std::time::Duration;

use niri_display_manager::domain::profile::{self, ProfileKind};
use niri_display_manager::infrastructure::drm_sysfs::{HardwareProbe, SysfsProbe};
use niri_display_manager::infrastructure::kdl_parser;
use niri_display_manager::infrastructure::niri_ipc::{NiriCliClient, NiriClient};
use niri_display_manager::infrastructure::process_runner::{
    CommandSpec, ProcessSupervisor, SystemCommandRunner, SystemSupervisor,
};
use niri_display_manager::service::display_service::is_mirror_window;

fn live_client() -> NiriCliClient {
    NiriCliClient::new(Arc::new(SystemCommandRunner))
}

#[test]
#[ignore = "requires a live niri session"]
fn live_niri_reports_outputs() {
    let outputs = live_client()
        .outputs()
        .expect("niri must answer in a live session");
    assert!(!outputs.is_empty(), "at least one output is expected");
    for output in &outputs {
        assert!(!output.name.is_empty());
    }
}

#[test]
#[ignore = "requires a live niri session"]
fn live_sysfs_probe_reports_connectors() {
    let connectors = SysfsProbe::system()
        .connectors()
        .expect("DRM sysfs must be readable");
    assert!(
        !connectors.is_empty(),
        "at least one DRM connector is expected"
    );
    for connector in &connectors {
        assert!(connector.card.starts_with("card"));
        assert!(!connector.connector.is_empty());
    }
}

#[test]
#[ignore = "requires a live niri session and wl-mirror"]
fn live_mirror_window_is_detected() {
    let client = live_client();
    let outputs = client
        .outputs()
        .expect("niri must answer in a live session");
    let source = outputs
        .iter()
        .find(|output| output.is_internal())
        .or_else(|| outputs.first())
        .expect("at least one output is required")
        .name
        .clone();

    let supervisor = SystemSupervisor::new();
    let spec = CommandSpec::new("wl-mirror", vec![source]);
    let pid = supervisor.spawn(&spec).expect("wl-mirror must spawn");
    let mut detected = false;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let windows = client.windows().expect("niri must answer");
        if windows.iter().any(|window| is_mirror_window(window, pid)) {
            detected = true;
            break;
        }
    }
    supervisor
        .terminate(Duration::from_secs(2))
        .expect("wl-mirror must terminate");
    assert!(
        detected,
        "wl-mirror window must be detected by pid or app id"
    );
}

#[test]
#[ignore = "requires a live niri session"]
fn live_generated_configurations_pass_niri_validation() {
    let client = live_client();
    let outputs = client
        .outputs()
        .expect("niri must answer in a live session");
    let connectors = SysfsProbe::system()
        .connectors()
        .expect("DRM sysfs must be readable");

    let directory = tempfile::tempdir().expect("temporary directory must be creatable");
    let config_path = directory.path().join("generated-display.kdl");

    for profile in ProfileKind::ALL {
        let plan = match profile::plan_profile(profile, &outputs, &connectors) {
            Ok(plan) => plan,
            // Mirror and External Only legitimately require a connected
            // external display; skip them when the hardware is not attached.
            Err(error) => {
                eprintln!("skipping {profile}: {error}");
                continue;
            }
        };
        let section = kdl_parser::render_section(&plan);
        std::fs::write(&config_path, &section).expect("generated config must be writable");
        client
            .validate_config(&config_path)
            .unwrap_or_else(|error| {
                panic!("niri rejected generated config for {profile}: {error}")
            });
    }
}
