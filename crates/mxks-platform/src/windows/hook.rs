//! Global capture via a `WH_KEYBOARD_LL` low-level keyboard hook.
//!
//! The hook callback is a plain C function, so shared state lives in a process
//! global set before the hook is installed. Injected events (tagged with
//! `dwExtraInfo == MAGIC`) are ignored here, which is race-free.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use crate::{HotkeyControl, InterceptControl};
use anyhow::Result;
use crossbeam_channel::Sender;
use mxks_core::hotkey::HotkeySpec;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
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
    hotkey: HotkeyControl,
    intercept: InterceptControl,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// VK whose next key-up must be swallowed (we ate its key-down as `Accept`);
/// 0 when none. Keeps apps from seeing an orphan key-up.
static SWALLOW_UP_VK: AtomicU32 = AtomicU32::new(0);

/// One configured-hotkey press is emitted on its physical key-up. Holding the
/// key may produce many key-down messages, but all of them share this one
/// pending VK and therefore cannot trigger repeated conversions.
struct HotkeyEdgeState {
    pending_vk: AtomicU32,
}

impl HotkeyEdgeState {
    const fn new() -> Self {
        Self {
            pending_vk: AtomicU32::new(0),
        }
    }

    /// Returns true when this key-down belongs to the armed hotkey press and
    /// must not be classified or emitted. `matches` is evaluated by the hook
    /// only from the modifiers present on key-down.
    fn consume_key_down(&self, vk: u32, matches: bool) -> bool {
        if vk == 0 {
            return false;
        }
        if self.pending_vk.load(Ordering::SeqCst) == vk {
            return true;
        }
        if !matches {
            return false;
        }
        let _ = self
            .pending_vk
            .compare_exchange(0, vk, Ordering::SeqCst, Ordering::SeqCst);
        true
    }

    /// Consumes only the key-up for the VK armed on key-down. No modifier
    /// state is consulted here, so releasing configured modifiers first does
    /// not lose the hotkey.
    fn emit_on_key_up(&self, vk: u32) -> bool {
        vk != 0
            && self
                .pending_vk
                .compare_exchange(vk, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

static HOTKEY_EDGE: HotkeyEdgeState = HotkeyEdgeState::new();

pub struct WinCapture {
    hotkey: HotkeyControl,
    intercept: InterceptControl,
}

impl WinCapture {
    pub fn new(hotkey: HotkeyControl, intercept: InterceptControl) -> Self {
        WinCapture { hotkey, intercept }
    }
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

#[derive(Clone, Copy)]
struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
}

fn current_mods() -> Mods {
    Mods {
        shift: key_down(VK_SHIFT),
        ctrl: key_down(VK_CONTROL),
        alt: key_down(VK_MENU),
        meta: key_down(VK_LWIN) || key_down(VK_RWIN),
    }
}

/// True if `vk` is a modifier key; pressing it alone must not reset the buffer.
fn is_modifier_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0x14 | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

/// Ignore the phantom Control that a Pause key may carry.
fn norm_mods(name: &Option<String>, m: &Mods) -> Mods {
    let mut m = *m;
    if matches!(name.as_deref(), Some("PAUSE")) {
        m.ctrl = false;
    }
    m
}

/// Canonical hotkey name for a key: letters via scan code, named keys via VK.
fn name_of(kb: &KBDLLHOOKSTRUCT) -> Option<String> {
    if let Some(name) = keymap::key_letter_name(kb.scanCode) {
        return Some(name);
    }
    keymap::vk_name(kb.vkCode as u16)
}

fn capture_spec(kb: &KBDLLHOOKSTRUCT, m: &Mods) -> Option<HotkeySpec> {
    let name = name_of(kb)?;
    let m = norm_mods(&Some(name.clone()), m);
    let has_mod = m.ctrl || m.alt || m.meta;
    if name.len() == 1 && !has_mod {
        return None; // avoid a bare-letter hotkey
    }
    Some(HotkeySpec {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
        key: name,
    })
}

fn matches_spec(spec: &HotkeySpec, kb: &KBDLLHOOKSTRUCT, m: &Mods) -> bool {
    match name_of(kb) {
        Some(name) => {
            let ignore_ctrl = name.eq_ignore_ascii_case("PAUSE");
            name.eq_ignore_ascii_case(&spec.key)
                && (ignore_ctrl || m.ctrl == spec.ctrl)
                && m.shift == spec.shift
                && m.alt == spec.alt
                && m.meta == spec.meta
        }
        None => false,
    }
}

fn matches_hotkey(shared: &Shared, kb: &KBDLLHOOKSTRUCT, m: &Mods) -> bool {
    matches_spec(&shared.hotkey.current(), kb, m)
}

fn classify(kb: &KBDLLHOOKSTRUCT, m: &Mods) -> Option<KeyKind> {
    let scan = kb.scanCode;

    if is_modifier_vk(kb.vkCode as u16) {
        return None;
    }
    if m.ctrl || m.alt || m.meta {
        return Some(KeyKind::Reset);
    }
    if let Some(key) = keymap::phys_of(scan) {
        return Some(KeyKind::Letter {
            key,
            shift: m.shift,
        });
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

/// `dwExtraInfo` is the precise marker for our own `SendInput` calls, but some
/// Windows/application paths do not preserve it. `LLKHF_INJECTED` is set by the
/// OS for every synthetic keyboard event and is the authoritative fallback.
fn is_injected(kb: &KBDLLHOOKSTRUCT) -> bool {
    kb.dwExtraInfo == MAGIC || kb.flags.contains(LLKHF_INJECTED)
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Never feed synthetic input back into the word buffer. Prefer our
        // MAGIC tag, with the OS-injected flag as a required fallback.
        if !is_injected(kb) {
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                if let Some(shared) = SHARED.get() {
                    let m = current_mods();
                    // Accept-key interception: swallow the key while a
                    // suggestion is visible (single atomic load when idle).
                    if shared.intercept.is_active()
                        && matches_spec(&shared.intercept.current(), kb, &m)
                    {
                        let _ = shared.tx.send(KeyEvent {
                            kind: KeyKind::Accept,
                            down: true,
                            injected: false,
                        });
                        SWALLOW_UP_VK.store(kb.vkCode, Ordering::SeqCst);
                        return LRESULT(1);
                    }
                    if shared.hotkey.is_capturing() {
                        if let Some(spec) = capture_spec(kb, &m) {
                            shared.hotkey.record(spec);
                        }
                    } else {
                        let matches = matches_hotkey(shared, kb, &m);
                        if !HOTKEY_EDGE.consume_key_down(kb.vkCode, matches) {
                            if let Some(kind) = classify(kb, &m) {
                                let _ = shared.tx.send(KeyEvent {
                                    kind,
                                    down: true,
                                    injected: false,
                                });
                            }
                        }
                    }
                }
            } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                if kb.vkCode != 0 && SWALLOW_UP_VK.load(Ordering::SeqCst) == kb.vkCode {
                    SWALLOW_UP_VK.store(0, Ordering::SeqCst);
                    return LRESULT(1);
                }
                if HOTKEY_EDGE.emit_on_key_up(kb.vkCode) {
                    if let Some(shared) = SHARED.get() {
                        let _ = shared.tx.send(KeyEvent {
                            kind: KeyKind::Hotkey,
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
        let _ = SHARED.set(Shared {
            tx,
            hotkey: self.hotkey.clone(),
            intercept: self.intercept.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_flag_suppresses_event_when_extra_info_is_lost() {
        let kb = KBDLLHOOKSTRUCT {
            flags: LLKHF_INJECTED,
            ..Default::default()
        };

        assert!(is_injected(&kb));
    }

    #[test]
    fn magic_tag_suppresses_event_without_injected_flag() {
        let kb = KBDLLHOOKSTRUCT {
            dwExtraInfo: MAGIC,
            ..Default::default()
        };

        assert!(is_injected(&kb));
    }

    #[test]
    fn physical_event_is_not_suppressed() {
        assert!(!is_injected(&KBDLLHOOKSTRUCT::default()));
    }

    #[test]
    fn down_repeats_and_up_emit_one_hotkey() {
        let state = HotkeyEdgeState::new();
        let vk = 0x13;

        assert!(state.consume_key_down(vk, true));
        assert!(state.consume_key_down(vk, true));
        assert!(state.consume_key_down(vk, true));
        assert!(state.emit_on_key_up(vk));
        assert!(!state.emit_on_key_up(vk));
    }

    #[test]
    fn unrelated_keyup_does_not_consume_pending_hotkey() {
        let state = HotkeyEdgeState::new();
        let vk = 0x13;

        assert!(state.consume_key_down(vk, true));
        assert!(!state.emit_on_key_up(0x20));
        assert!(state.emit_on_key_up(vk));
    }

    #[test]
    fn second_physical_press_emits_second_hotkey() {
        let state = HotkeyEdgeState::new();
        let vk = 0x13;

        assert!(state.consume_key_down(vk, true));
        assert!(state.emit_on_key_up(vk));
        assert!(state.consume_key_down(vk, true));
        assert!(state.emit_on_key_up(vk));
    }

    #[test]
    fn unmatched_keydown_never_arms() {
        let state = HotkeyEdgeState::new();
        let vk = 0x13;

        assert!(!state.consume_key_down(vk, false));
        assert!(!state.emit_on_key_up(vk));
    }

    #[test]
    fn modifier_config_is_matched_only_on_keydown() {
        let state = HotkeyEdgeState::new();
        let vk = 0x13;

        assert!(state.consume_key_down(vk, true));
        assert!(state.consume_key_down(vk, false));
        assert!(state.emit_on_key_up(vk));

        assert!(!state.consume_key_down(vk, false));
        assert!(!state.emit_on_key_up(vk));
    }
}
