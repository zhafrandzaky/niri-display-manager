//! Presentation layer: GTK4 / libadwaita user interface.
//!
//! The UI is a pure function of [`view_model::AppState`]: it renders plain row
//! structures and emits [`view_model::AppAction`] values. All I/O happens on
//! the worker thread inside the service layer.

pub mod view_model;
pub mod views;

pub use views::main_window::run;
