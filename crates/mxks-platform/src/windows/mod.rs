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
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
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

/// Best-effort focus info. Password-field detection is not yet implemented on
/// Windows (planned via UI Automation); the per-app mode is driven by the
/// foreground window's process executable name.
struct WinFocus;

impl FocusInfo for WinFocus {
    /// Lowercased executable basename of the foreground window's process,
    /// e.g. "telegram.exe" — the Windows analog of the X11 WM_CLASS.
    fn focused_app(&self) -> Option<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            result.ok()?;
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            let base = full.rsplit(['\\', '/']).next()?;
            Some(base.trim().to_lowercase())
        }
    }
}
