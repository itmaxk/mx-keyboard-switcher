//! `mxks-core` — pure, OS-independent logic for MX Keyboard Switcher:
//! physical-key model, layout tables, conversion, the word buffer, the
//! wrong-layout detector, and the config schema.
//!
//! This crate has no platform dependencies so it can be unit-tested identically
//! on every target.

pub mod buffer;
pub mod config;
pub mod convert;
pub mod detect;
pub mod dict;
pub mod hotkey;
pub mod keycode;
pub mod layout;
pub mod usage;

/// Tables generated at build time from `data/` (word lists + bigram models).
pub mod tables {
    include!(concat!(env!("OUT_DIR"), "/tables.rs"));
}

pub use convert::{convert_str, convert_str_to, render_keys, Stroke};
pub use keycode::PhysKey;
pub use layout::Lang;
