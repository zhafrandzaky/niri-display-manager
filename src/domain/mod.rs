//! Domain layer: pure entities and business rules.
//!
//! Types here must not perform I/O and must not depend on niri, GTK, or the
//! filesystem. Layout planning and verification live in [`profile`].

pub mod display;
pub mod profile;
