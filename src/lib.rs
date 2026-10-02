//! niri-display-manager: display and mirroring manager for the niri compositor.
//!
//! The crate is organized in four layers with a strict dependency direction:
//!
//! - [`domain`]: pure entities and layout rules; performs no I/O.
//! - [`service`]: application use cases orchestrating domain logic through traits.
//! - [`infrastructure`]: adapters for niri IPC, DRM sysfs, config files, and processes.
//! - [`presentation`]: GTK4/libadwaita user interface and worker wiring.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cli;
pub mod domain;
pub mod infrastructure;
pub mod presentation;
pub mod service;

/// Application identity shared by the D-Bus name and the desktop entry.
pub const APP_ID: &str = "io.github.zyy.NiriDisplayManager";
