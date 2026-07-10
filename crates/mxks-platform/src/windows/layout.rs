//! Reading and switching the active keyboard layout via Win32.

use anyhow::Result;
use mxks_core::layout::Lang;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, LoadKeyboardLayoutW, ACTIVATE_KEYBOARD_LAYOUT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

const LANGID_EN: u16 = 0x0409;
const LANGID_RU: u16 = 0x0419;
const KLF_ACTIVATE: u32 = 0x0000_0001;

pub struct WinLayout;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl crate::LayoutSwitcher for WinLayout {
    fn current(&self) -> Result<Option<Lang>> {
        unsafe {
            let hwnd = GetForegroundWindow();
            let tid = GetWindowThreadProcessId(hwnd, None);
            let hkl = GetKeyboardLayout(tid);
            let langid = (hkl.0 as usize & 0xFFFF) as u16;
            Ok(match langid {
                LANGID_EN => Some(Lang::En),
                LANGID_RU => Some(Lang::Ru),
                _ => None,
            })
        }
    }

    fn switch_to(&mut self, lang: Lang) -> Result<()> {
        let klid = match lang {
            Lang::En => "00000409",
            Lang::Ru => "00000419",
        };
        let wide = to_wide(klid);
        unsafe {
            let hkl = LoadKeyboardLayoutW(
                PCWSTR(wide.as_ptr()),
                ACTIVATE_KEYBOARD_LAYOUT_FLAGS(KLF_ACTIVATE),
            )?;
            let hwnd = GetForegroundWindow();
            // Ask the focused window to switch layout (more reliable than
            // ActivateKeyboardLayout, which affects only the calling thread).
            PostMessageW(
                Some(hwnd),
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(hkl.0 as isize),
            )?;
        }
        Ok(())
    }
}
