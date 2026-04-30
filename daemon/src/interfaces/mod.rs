//! D-Bus interface implementations.
//!
//! One module per portal interface. Each module defines a struct, the
//! `#[zbus::interface]` impl, and any helpers private to that interface.

pub mod file_chooser;
pub mod open_uri;
pub mod options;
