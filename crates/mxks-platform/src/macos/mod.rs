//! macOS backend: `CGEventTap` capture, `CGEvent` Unicode injection, and TIS
//! layout switching. Injected events are tagged via the event source's user
//! data field so the tap can ignore them.
//!
//! Requires the **Accessibility** permission to receive events; the tap simply
//! yields nothing until it is granted.

mod inject;
mod keymap;
mod layout;
mod tap;

use anyhow::Result;
use mxks_core::hotkey::HotkeySpec;

use crate::{Backend, FocusInfo};

/// Tag written to injected events' user-data field.
pub const MAGIC: i64 = 0x4B42_5357; // "KBSW"

pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    let (control, handle) = crate::hotkey_channel(hotkey);
    let (icontrol, ihandle) = crate::intercept_channel(crate::default_accept());
    Ok(Backend {
        capture: Box::new(tap::MacCapture::new(control, icontrol)),
        injector: Box::new(inject::MacInjector::new()?),
        layout: Box::new(layout::MacLayout),
        focus: Box::new(MacFocus),
        hotkey: handle,
        intercept: ihandle,
        // No overlay on macOS yet (needs an NSPanel via objc2); the stub keeps
        // autocomplete inert here.
        overlay: Box::new(crate::StubOverlay),
    })
}

/// macOS blinds event taps automatically while Secure Input is active (password
/// fields), so no explicit password-field check is needed here.
struct MacFocus;
impl FocusInfo for MacFocus {}
