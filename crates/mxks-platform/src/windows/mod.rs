//! Windows backend: `WH_KEYBOARD_LL` capture, `SendInput` injection, and
//! layout read/switch via Win32. Injected events are tagged with `MAGIC` in
//! `dwExtraInfo` so the hook ignores them.

mod hook;
mod inject;
mod keymap;
mod layout;
mod overlay;

use anyhow::Result;
use mxks_core::hotkey::HotkeySpec;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, HKL};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::{Backend, FocusInfo};

/// Tag written to `dwExtraInfo` on every injected event.
pub const MAGIC: usize = 0x4B42_5357; // "KBSW"

pub(super) struct ForegroundKeyboard {
    pub hwnd: HWND,
    pub thread_id: u32,
    pub hkl: HKL,
}

pub(super) fn foreground_keyboard() -> Result<ForegroundKeyboard> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            anyhow::bail!("no foreground window");
        }

        let thread_id = GetWindowThreadProcessId(hwnd, None);
        if thread_id == 0 {
            anyhow::bail!("foreground window has no thread");
        }

        let hkl = GetKeyboardLayout(thread_id);
        if hkl.0.is_null() {
            anyhow::bail!("foreground thread has no keyboard layout");
        }

        Ok(ForegroundKeyboard {
            hwnd,
            thread_id,
            hkl,
        })
    }
}

pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    let (control, handle) = crate::hotkey_channel(hotkey);
    let (icontrol, ihandle) = crate::intercept_channel(crate::default_accept());
    Ok(Backend {
        capture: Box::new(hook::WinCapture::new(control, icontrol)),
        injector: Box::new(inject::WinInjector),
        layout: Box::new(layout::WinLayout),
        focus: Box::new(WinFocus),
        hotkey: handle,
        intercept: ihandle,
        overlay: Box::new(overlay::WinOverlay),
    })
}

/// Best-effort focus info. Password-field and per-app detection are not yet
/// implemented on Windows (planned via UI Automation); v1 relies on the
/// content gates in the detector.
struct WinFocus;
impl FocusInfo for WinFocus {}
