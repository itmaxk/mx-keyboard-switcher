//! Input injection via `SendInput`. Every injected event carries `dwExtraInfo
//! == MAGIC` so the low-level hook can recognize and ignore it (clean, race-free
//! self-event suppression).

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, MapVirtualKeyExW, SendInput, VkKeyScanExW, HKL, INPUT, INPUT_0,
    INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC_EX, VIRTUAL_KEY,
    VK_BACK, VK_CAPITAL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB,
};

use super::MAGIC;

const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(2);
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(250);

pub struct WinInjector;

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        bail!("SendInput sent {sent}/{} events", inputs.len());
    }
    Ok(())
}

fn append_vk_pair(inputs: &mut Vec<INPUT>, vk: VIRTUAL_KEY) {
    inputs.push(key_input(vk.0, 0, KEYBD_EVENT_FLAGS(0)));
    inputs.push(key_input(vk.0, 0, KEYEVENTF_KEYUP));
}

fn append_unicode_unit(inputs: &mut Vec<INPUT>, unit: u16) {
    inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
    inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
}

fn scan_parts(mapped: u32) -> Option<(u16, KEYBD_EVENT_FLAGS)> {
    let scan = (mapped & 0xff) as u16;
    if scan == 0 {
        return None;
    }
    let prefix = mapped & 0xff00;
    let extended = if prefix == 0xe000 || prefix == 0xe100 {
        KEYEVENTF_EXTENDEDKEY
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    Some((scan, extended))
}

fn append_scan_pair(inputs: &mut Vec<INPUT>, scan: u16, extra_flags: KEYBD_EVENT_FLAGS) {
    let down_flags = KEYEVENTF_SCANCODE | extra_flags;
    inputs.push(key_input(0, scan, down_flags));
    inputs.push(key_input(0, scan, down_flags | KEYEVENTF_KEYUP));
}

fn append_physical_char(
    inputs: &mut Vec<INPUT>,
    ch: char,
    unit: u16,
    hkl: HKL,
    caps_lock: bool,
) -> bool {
    let mapping = unsafe { VkKeyScanExW(unit, hkl) };
    if mapping == -1 {
        return false;
    }

    let vk = (mapping as u16 & 0xff) as u32;
    let modifiers = ((mapping as u16 >> 8) & 0xff) as u8;
    if modifiers & !1 != 0 {
        return false;
    }
    let mapped_char = unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, Some(hkl)) };
    if mapped_char & 0x8000_0000 != 0 {
        return false;
    }
    let Some((scan, extra_flags)) =
        scan_parts(unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC_EX, Some(hkl)) })
    else {
        return false;
    };

    let required_shift = modifiers & 1 != 0;
    let use_shift = if ch.is_alphabetic() {
        required_shift ^ caps_lock
    } else {
        required_shift
    };
    let shift_scan = if use_shift {
        scan_parts(unsafe { MapVirtualKeyExW(VK_SHIFT.0 as u32, MAPVK_VK_TO_VSC_EX, Some(hkl)) })
    } else {
        None
    };
    if use_shift && shift_scan.is_none() {
        return false;
    }

    if let Some((shift_scan, shift_flags)) = shift_scan {
        append_scan_pair_down(inputs, shift_scan, shift_flags);
    }
    append_scan_pair(inputs, scan, extra_flags);
    if let Some((shift_scan, shift_flags)) = shift_scan {
        append_scan_pair_up(inputs, shift_scan, shift_flags);
    }
    true
}

fn append_scan_pair_down(inputs: &mut Vec<INPUT>, scan: u16, extra_flags: KEYBD_EVENT_FLAGS) {
    inputs.push(key_input(0, scan, KEYEVENTF_SCANCODE | extra_flags));
}

fn append_scan_pair_up(inputs: &mut Vec<INPUT>, scan: u16, extra_flags: KEYBD_EVENT_FLAGS) {
    inputs.push(key_input(
        0,
        scan,
        KEYEVENTF_SCANCODE | extra_flags | KEYEVENTF_KEYUP,
    ));
}

fn append_text_inputs(inputs: &mut Vec<INPUT>, text: &str, hkl: HKL, caps_lock: bool) {
    for ch in text.chars() {
        let mut units = [0; 2];
        let encoded = ch.encode_utf16(&mut units);
        if encoded.len() == 1 && append_physical_char(inputs, ch, encoded[0], hkl, caps_lock) {
            continue;
        }
        for unit in encoded {
            append_unicode_unit(inputs, *unit);
        }
    }
}

fn text_inputs(text: &str, hkl: HKL, caps_lock: bool) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 4);
    append_text_inputs(&mut inputs, text, hkl, caps_lock);
    inputs
}

fn replacement_inputs(
    erase: usize,
    text: &str,
    trailing: &str,
    hkl: HKL,
    caps_lock: bool,
) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(
        erase
            .saturating_mul(2)
            .saturating_add((text.len() + trailing.len()).saturating_mul(4)),
    );
    for _ in 0..erase {
        append_vk_pair(&mut inputs, VK_BACK);
    }
    append_text_inputs(&mut inputs, text, hkl, caps_lock);
    append_text_inputs(&mut inputs, trailing, hkl, caps_lock);
    inputs
}

fn wait_for_modifiers_released() -> Result<()> {
    const MODIFIERS: [VIRTUAL_KEY; 8] = [
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ];
    let deadline = Instant::now() + MODIFIER_RELEASE_TIMEOUT;
    loop {
        let held = MODIFIERS
            .iter()
            .any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } < 0);
        if !held {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("modifier keys remained held during injection");
        }
        std::thread::sleep(MODIFIER_POLL_INTERVAL);
    }
}

fn foreground_layout() -> Result<(HKL, bool)> {
    let keyboard = super::foreground_keyboard()?;
    let caps_lock = unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0;
    Ok((keyboard.hkl, caps_lock))
}

impl crate::KeyInjector for WinInjector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        let mut inputs = Vec::with_capacity(n.saturating_mul(2));
        for _ in 0..n {
            append_vk_pair(&mut inputs, VK_BACK);
        }
        if !inputs.is_empty() {
            send(&inputs)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        wait_for_modifiers_released()?;
        let (hkl, caps_lock) = foreground_layout()?;
        let inputs = text_inputs(text, hkl, caps_lock);
        send(&inputs)
    }

    fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
        if erase == 0 && text.is_empty() && trailing.is_empty() {
            return Ok(());
        }
        wait_for_modifiers_released()?;
        let (hkl, caps_lock) = foreground_layout()?;
        let inputs = replacement_inputs(erase, text, trailing, hkl, caps_lock);
        send(&inputs)
    }

    fn tab(&mut self) -> Result<()> {
        send(&[
            key_input(VK_TAB.0, 0, KEYBD_EVENT_FLAGS(0)),
            key_input(VK_TAB.0, 0, KEYEVENTF_KEYUP),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::UI::Input::KeyboardAndMouse::{LoadKeyboardLayoutW, KLF_SUBSTITUTE_OK};

    fn load_layout(klid: windows::core::PCWSTR) -> HKL {
        unsafe { LoadKeyboardLayoutW(klid, KLF_SUBSTITUTE_OK).unwrap() }
    }

    fn keyboard(input: &INPUT) -> KEYBDINPUT {
        unsafe { input.Anonymous.ki }
    }

    fn assert_tagged(inputs: &[INPUT]) {
        assert!(inputs
            .iter()
            .all(|input| keyboard(input).dwExtraInfo == MAGIC));
    }

    fn assert_no_unicode(inputs: &[INPUT]) {
        assert!(inputs
            .iter()
            .all(|input| keyboard(input).dwFlags.0 & KEYEVENTF_UNICODE.0 == 0));
    }

    #[test]
    fn russian_word_uses_one_scancode_pair_per_letter() {
        let inputs = text_inputs("капельки", load_layout(w!("00000419")), false);
        assert_eq!(inputs.len(), 16);
        assert_no_unicode(&inputs);
        assert_tagged(&inputs);
        for pair in inputs.chunks_exact(2) {
            let down = keyboard(&pair[0]);
            let up = keyboard(&pair[1]);
            assert_eq!(down.wVk.0, 0);
            assert_ne!(down.dwFlags.0 & KEYEVENTF_SCANCODE.0, 0);
            assert_eq!(down.wScan, up.wScan);
            assert_eq!(up.dwFlags.0, down.dwFlags.0 | KEYEVENTF_KEYUP.0);
        }
    }

    #[test]
    fn caps_lock_preserves_mixed_case_without_unicode() {
        let hkl = load_layout(w!("00000419"));
        let caps_off = text_inputs("Привет", hkl, false);
        let caps_on = text_inputs("Привет", hkl, true);
        assert_no_unicode(&caps_off);
        assert_no_unicode(&caps_on);
        assert_eq!(caps_off.len(), 14);
        assert_eq!(caps_on.len(), 22);
        assert_tagged(&caps_off);
        assert_tagged(&caps_on);
    }

    #[test]
    fn yo_and_space_use_physical_keys() {
        let inputs = text_inputs("ё ", load_layout(w!("00000419")), false);
        assert_eq!(inputs.len(), 4);
        assert_no_unicode(&inputs);
        assert_tagged(&inputs);
    }

    #[test]
    fn surrogate_pair_falls_back_to_complete_unicode_lifecycles() {
        let inputs = text_inputs("💧", load_layout(w!("00000409")), false);
        assert_eq!(inputs.len(), 4);
        assert_tagged(&inputs);
        for pair in inputs.chunks_exact(2) {
            let down = keyboard(&pair[0]);
            let up = keyboard(&pair[1]);
            assert_eq!(down.wVk.0, 0);
            assert_eq!(down.wScan, up.wScan);
            assert_eq!(down.dwFlags, KEYEVENTF_UNICODE);
            assert_eq!(up.dwFlags, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        }
    }

    #[test]
    fn replacement_is_one_tagged_batch() {
        let inputs = replacement_inputs(9, "капельки", " ", load_layout(w!("00000419")), false);
        assert_eq!(inputs.len(), 36);
        assert_tagged(&inputs);
        assert_no_unicode(&inputs);
        for pair in inputs[..18].chunks_exact(2) {
            assert_eq!(keyboard(&pair[0]).wVk, VK_BACK);
            assert_eq!(keyboard(&pair[1]).wVk, VK_BACK);
            assert_eq!(
                keyboard(&pair[1]).dwFlags,
                keyboard(&pair[0]).dwFlags | KEYEVENTF_KEYUP
            );
        }
    }
}
