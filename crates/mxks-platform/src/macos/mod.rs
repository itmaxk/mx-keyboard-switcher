//! macOS backend: `CGEventTap` capture, `CGEvent` Unicode injection, and TIS
//! layout switching. Injected events are tagged via the event source's user
//! data field so the tap can ignore them.
//!
//! Requires the **Accessibility** permission to receive events; the tap simply
//! yields nothing until it is granted.

mod focus;
mod inject;
mod keymap;
mod layout;
mod tap;

use anyhow::Result;
use mxks_core::hotkey::HotkeySpec;

use crate::Backend;

/// Tag written to injected events' user-data field.
pub const MAGIC: i64 = 0x4B42_5357; // "KBSW"

pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    let (control, handle) = crate::hotkey_channel(hotkey);
    let (icontrol, ihandle) = crate::intercept_channel(crate::default_accept());
    Ok(Backend {
        capture: Box::new(tap::MacCapture::new(control, icontrol)),
        injector: Box::new(inject::MacInjector::new()?),
        layout: Box::new(layout::MacLayout),
        focus: Box::new(focus::MacFocus::default()),
        hotkey: handle,
        intercept: ihandle,
        // No overlay on macOS yet (needs an NSPanel via objc2); the stub keeps
        // autocomplete inert here.
        overlay: Box::new(crate::StubOverlay),
    })
}
