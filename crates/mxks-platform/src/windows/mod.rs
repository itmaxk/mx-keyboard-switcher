//! Windows backend: `WH_KEYBOARD_LL` capture, `SendInput` injection, and
//! layout read/switch via Win32. Injected events are tagged with `MAGIC` in
//! `dwExtraInfo` so the hook ignores them.

mod hook;
mod inject;
mod keymap;
mod layout;

use anyhow::Result;
use mxks_core::hotkey::HotkeySpec;

use crate::{Backend, FocusInfo};

/// Tag written to `dwExtraInfo` on every injected event.
pub const MAGIC: usize = 0x4B42_5357; // "KBSW"

pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    Ok(Backend {
        capture: Box::new(hook::WinCapture::new(hotkey)),
        injector: Box::new(inject::WinInjector),
        layout: Box::new(layout::WinLayout),
        focus: Box::new(WinFocus),
    })
}

/// Best-effort focus info. Password-field and per-app detection are not yet
/// implemented on Windows (planned via UI Automation); v1 relies on the
/// content gates in the detector.
struct WinFocus;
impl FocusInfo for WinFocus {}
