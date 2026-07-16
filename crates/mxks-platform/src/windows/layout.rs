//! Reading and switching the active keyboard layout via Win32.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use mxks_core::layout::Lang;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, LoadKeyboardLayoutW, ACTIVATE_KEYBOARD_LAYOUT_FLAGS, KLF_ACTIVATE,
    KLF_SUBSTITUTE_OK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_INPUTLANGCHANGEREQUEST,
};

const LANGID_EN: u16 = 0x0409;
const LANGID_RU: u16 = 0x0419;
const SWITCH_TIMEOUT: Duration = Duration::from_millis(10);
const POLL_INTERVAL: Duration = Duration::from_millis(1);

pub struct WinLayout;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl crate::LayoutSwitcher for WinLayout {
    fn current(&self) -> Result<Option<Lang>> {
        let keyboard = super::foreground_keyboard()?;
        Ok(match langid(keyboard.hkl) {
            LANGID_EN => Some(Lang::En),
            LANGID_RU => Some(Lang::Ru),
            _ => None,
        })
    }

    fn switch_to(&mut self, lang: Lang) -> Result<()> {
        let target_langid = match lang {
            Lang::En => LANGID_EN,
            Lang::Ru => LANGID_RU,
        };
        let keyboard = super::foreground_keyboard()?;
        if langid(keyboard.hkl) == target_langid {
            return Ok(());
        }

        let klid = match lang {
            Lang::En => "00000409",
            Lang::Ru => "00000419",
        };
        let wide = to_wide(klid);
        let hkl = unsafe {
            LoadKeyboardLayoutW(
                PCWSTR(wide.as_ptr()),
                ACTIVATE_KEYBOARD_LAYOUT_FLAGS(KLF_ACTIVATE.0 | KLF_SUBSTITUTE_OK.0),
            )?
        };
        if langid(hkl) != target_langid {
            bail!("loaded keyboard layout has unexpected language");
        }

        let message_result = unsafe {
            SendMessageTimeoutW(
                keyboard.hwnd,
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(0),
                LPARAM(hkl.0 as isize),
                SMTO_ABORTIFHUNG,
                25,
                None,
            )
        };
        if message_result.0 == 0 {
            bail!("foreground window rejected or timed out changing keyboard layout");
        }

        let deadline = Instant::now() + SWITCH_TIMEOUT;
        loop {
            if unsafe { GetForegroundWindow() } != keyboard.hwnd {
                bail!("foreground window changed while activating keyboard layout");
            }
            let current = unsafe { GetKeyboardLayout(keyboard.thread_id) };
            if langid(current) == target_langid {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("target keyboard layout did not activate before deadline");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

fn langid(hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL) -> u16 {
    (hkl.0 as usize & 0xFFFF) as u16
}
