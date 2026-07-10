//! Global capture via a `WH_KEYBOARD_LL` low-level keyboard hook.
//!
//! The hook callback is a plain C function, so shared state lives in a process
//! global set before the hook is installed. Injected events (tagged with
//! `dwExtraInfo == MAGIC`) are ignored here, which is race-free.

use std::sync::OnceLock;

use anyhow::Result;
use crossbeam_channel::Sender;
use mxks_core::hotkey::HotkeySpec;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

use super::keymap;
use super::MAGIC;
use crate::event::{KeyEvent, KeyKind};

// Virtual-key codes for modifiers.
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12; // Alt
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;

struct Shared {
    tx: Sender<KeyEvent>,
    hotkey_vk: Option<u16>,
    hotkey: HotkeySpec,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

pub struct WinCapture {
    hotkey: HotkeySpec,
}

impl WinCapture {
    pub fn new(hotkey: HotkeySpec) -> Self {
        WinCapture { hotkey }
    }
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

fn classify(shared: &Shared, kb: &KBDLLHOOKSTRUCT) -> Option<KeyKind> {
    let scan = kb.scanCode;
    let vk = kb.vkCode as u16;

    let shift = key_down(VK_SHIFT);
    let ctrl = key_down(VK_CONTROL);
    let alt = key_down(VK_MENU);
    let meta = key_down(VK_LWIN) || key_down(VK_RWIN);

    if Some(vk) == shared.hotkey_vk
        && ctrl == shared.hotkey.ctrl
        && shift == shared.hotkey.shift
        && alt == shared.hotkey.alt
        && meta == shared.hotkey.meta
    {
        return Some(KeyKind::Hotkey);
    }

    if ctrl || alt || meta {
        return Some(KeyKind::Reset);
    }

    if let Some(key) = keymap::phys_of(scan) {
        return Some(KeyKind::Letter { key, shift });
    }
    if scan == keymap::SC_BACKSPACE {
        return Some(KeyKind::Backspace);
    }
    if scan == keymap::SC_SPACE {
        return Some(KeyKind::Boundary { sep: Some(' ') });
    }
    if keymap::is_boundary(scan) {
        return Some(KeyKind::Boundary { sep: None });
    }
    Some(KeyKind::Reset)
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Ignore our own injected events.
        if kb.dwExtraInfo != MAGIC {
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                if let Some(shared) = SHARED.get() {
                    if let Some(kind) = classify(shared, kb) {
                        let _ = shared.tx.send(KeyEvent {
                            kind,
                            down: true,
                            injected: false,
                        });
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

impl crate::KeyCapture for WinCapture {
    fn run(&mut self, tx: Sender<KeyEvent>) -> Result<()> {
        let hotkey_vk = keymap::vk_for_name(&self.hotkey.key);
        let _ = SHARED.set(Shared {
            tx,
            hotkey_vk,
            hotkey: self.hotkey.clone(),
        });

        unsafe {
            let hmod = GetModuleHandleW(None)?;
            let hook =
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(HINSTANCE(hmod.0)), 0)?;

            let mut msg = MSG::default();
            loop {
                let ret = GetMessageW(&mut msg, None, 0, 0);
                if ret.0 <= 0 {
                    break; // WM_QUIT (0) or error (-1)
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = UnhookWindowsHookEx(hook);
        }
        Ok(())
    }
}
