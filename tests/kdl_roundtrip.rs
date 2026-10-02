//! Integration tests for managed-section handling on real files.
//!
//! The manager must preserve every byte of user configuration outside the
//! managed fence across repeated writes, removals, and restores.

mod common;

use std::fs;

use common::store;
use niri_display_manager::domain::display::OutputId;
use niri_display_manager::domain::profile::{LayoutPlan, OutputPlan, ProfileKind};
use niri_display_manager::infrastructure::kdl_parser;
use niri_display_manager::service::backup_service::ConfigStore;

const USER_CONFIG: &str = r#"// User display configuration.
// Keep every comment and blank line below intact.

output "DP-1" {
    mode "2560x1440@359.979"
    scale 1
}

// A trailing note.
"#;

fn plan(profile: ProfileKind, external_x: i32) -> LayoutPlan {
    LayoutPlan {
        profile,
        outputs: vec![
            OutputPlan {
                output: OutputId::new("eDP-1"),
                enabled: true,
                position: Some((0, 0)),
                scale: None,
                expect_present: true,
            },
            OutputPlan {
                output: OutputId::new("HDMI-A-1"),
                enabled: true,
                position: Some((external_x, 0)),
                scale: Some(1.0),
                expect_present: true,
            },
        ],
        focus_output: None,
    }
}

#[test]
fn repeated_writes_replace_a_single_section_and_preserve_user_content() {
    let directory = tempfile::tempdir().unwrap();
    let config = store(directory.path());
    fs::create_dir_all(config.display_config_path().parent().unwrap()).unwrap();
    fs::write(config.display_config_path(), USER_CONFIG).unwrap();

    config.snapshot().unwrap();
    config
        .write_managed_section(&kdl_parser::render_section(&plan(
            ProfileKind::ExtendRight,
            1920,
        )))
        .unwrap();
    config
        .write_managed_section(&kdl_parser::render_section(&plan(
            ProfileKind::ExtendLeft,
            -1920,
        )))
        .unwrap();

    let written = fs::read_to_string(config.display_config_path()).unwrap();
    assert!(written.starts_with(USER_CONFIG));
    assert_eq!(written.matches(kdl_parser::BEGIN_MARKER).count(), 1);
    assert_eq!(written.matches(kdl_parser::END_MARKER).count(), 1);
    assert!(written.contains("// profile: extend-left"));
    assert!(!written.contains("// profile: extend-right"));
    assert!(written.contains("position x=-1920 y=0"));
    assert!(!directory.path().join("niri/cfg/display.kdl.tmp").exists());
}

#[test]
fn restore_returns_the_file_to_its_exact_original_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = store(directory.path());
    fs::create_dir_all(config.display_config_path().parent().unwrap()).unwrap();
    fs::write(config.display_config_path(), USER_CONFIG).unwrap();

    config.snapshot().unwrap();
    config
        .write_managed_section(&kdl_parser::render_section(&plan(
            ProfileKind::ExtendRight,
            1920,
        )))
        .unwrap();
    assert_ne!(
        fs::read_to_string(config.display_config_path()).unwrap(),
        USER_CONFIG
    );

    config.restore_snapshot().unwrap();
    assert_eq!(
        fs::read_to_string(config.display_config_path()).unwrap(),
        USER_CONFIG
    );

    // Idempotent: restoring again keeps the file identical.
    config.restore_snapshot().unwrap();
    assert_eq!(
        fs::read_to_string(config.display_config_path()).unwrap(),
        USER_CONFIG
    );
}

#[test]
fn manual_edits_outside_the_fence_survive_manager_writes() {
    let directory = tempfile::tempdir().unwrap();
    let config = store(directory.path());
    fs::create_dir_all(config.display_config_path().parent().unwrap()).unwrap();
    fs::write(config.display_config_path(), USER_CONFIG).unwrap();

    config
        .write_managed_section(&kdl_parser::render_section(&plan(
            ProfileKind::ExtendRight,
            1920,
        )))
        .unwrap();

    // The user edits their own part of the file after the manager wrote.
    let current = fs::read_to_string(config.display_config_path()).unwrap();
    let edited = current.replace(
        "// A trailing note.",
        "// A trailing note edited by the user.",
    );
    fs::write(config.display_config_path(), &edited).unwrap();

    config
        .write_managed_section(&kdl_parser::render_section(&plan(
            ProfileKind::ExtendLeft,
            -1920,
        )))
        .unwrap();

    let final_contents = fs::read_to_string(config.display_config_path()).unwrap();
    assert!(final_contents.contains("// A trailing note edited by the user."));
    assert_eq!(final_contents.matches(kdl_parser::BEGIN_MARKER).count(), 1);
    assert!(final_contents.contains("// profile: extend-left"));
}
