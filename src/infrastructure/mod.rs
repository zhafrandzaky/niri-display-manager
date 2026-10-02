//! Infrastructure layer: adapters for external systems.
//!
//! Adapters implement the traits consumed by the service layer and contain all
//! process, filesystem, and IPC knowledge.

pub mod drm_sysfs;
pub mod kdl_parser;
pub mod niri_ipc;
pub mod process_runner;
