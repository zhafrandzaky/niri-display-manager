//! Service layer: application use cases.
//!
//! Services orchestrate domain planning and verification through
//! infrastructure traits. They contain no GTK code and never call syscalls
//! directly; every external interaction goes through an injected adapter so
//! the layer is fully testable with fakes.

pub mod backup_service;
pub mod display_service;
