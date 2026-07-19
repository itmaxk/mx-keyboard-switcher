//! Input injection via `SendInput`. Every injected event carries `dwExtraInfo
//! == MAGIC` so the low-level hook can recognize and ignore it (clean, race-free
//! self-event suppression).
//!
//! Text is typed as `KEYEVENTF_UNICODE` (VK_PACKET) units, NOT as physical
//! scancodes. Scancodes are translated by the *receiving application* using
//! whatever keyboard layout it has applied at that moment, and apps with their
//! own layout handling (Qt apps like Telegram Desktop) apply
//! `WM_INPUTLANGCHANGE` asynchronously — a correction injected right after the
//! switch was retyped in the OLD layout ("Vbh" instead of "Мир"). Unicode
//! injection delivers the exact characters regardless of any layout, while the
//! system layout switch (see `layout.rs`) still happens so the user's *next*
//! keystrokes come out in the right language.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
    VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_TAB,
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

fn append_text_inputs(inputs: &mut Vec<INPUT>, text: &str) {
    for unit in text.encode_utf16() {
        append_unicode_unit(inputs, unit);
    }
}

fn text_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    append_text_inputs(&mut inputs, text);
    inputs
}

fn replacement_inputs(erase: usize, text: &str, trailing: &str) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(
        erase
            .saturating_mul(2)
            .saturating_add((text.len() + trailing.len()).saturating_mul(2)),
    );
    for _ in 0..erase {
        append_vk_pair(&mut inputs, VK_BACK);
    }
    append_text_inputs(&mut inputs, text);
    append_text_inputs(&mut inputs, trailing);
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
        send(&text_inputs(text))
    }

    fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
        if erase == 0 && text.is_empty() && trailing.is_empty() {
            return Ok(());
        }
        wait_for_modifiers_released()?;
        send(&replacement_inputs(erase, text, trailing))
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

    fn keyboard(input: &INPUT) -> KEYBDINPUT {
        unsafe { input.Anonymous.ki }
    }

    fn assert_tagged(inputs: &[INPUT]) {
        assert!(inputs
            .iter()
            .all(|input| keyboard(input).dwExtraInfo == MAGIC));
    }

    fn assert_unicode_pairs(inputs: &[INPUT]) {
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
    fn russian_word_uses_one_unicode_pair_per_letter() {
        let inputs = text_inputs("капельки");
        assert_eq!(inputs.len(), 16);
        assert_tagged(&inputs);
        assert_unicode_pairs(&inputs);
    }

    #[test]
    fn mixed_case_is_exact_regardless_of_caps_state() {
        // Unicode units carry the exact character, so case never depends on
        // CapsLock or Shift.
        let inputs = text_inputs("Привет");
        assert_eq!(inputs.len(), 12);
        assert_tagged(&inputs);
        assert_unicode_pairs(&inputs);
        assert_eq!(keyboard(&inputs[0]).wScan, 'П' as u16);
    }

    #[test]
    fn surrogate_pair_emits_two_complete_unit_lifecycles() {
        let inputs = text_inputs("💧");
        assert_eq!(inputs.len(), 4);
        assert_tagged(&inputs);
        assert_unicode_pairs(&inputs);
    }

    #[test]
    fn replacement_is_one_tagged_batch() {
        let inputs = replacement_inputs(9, "капельки", " ");
        assert_eq!(inputs.len(), 36);
        assert_tagged(&inputs);
        for pair in inputs[..18].chunks_exact(2) {
            assert_eq!(keyboard(&pair[0]).wVk, VK_BACK);
            assert_eq!(keyboard(&pair[1]).wVk, VK_BACK);
            assert_eq!(
                keyboard(&pair[1]).dwFlags,
                keyboard(&pair[0]).dwFlags | KEYEVENTF_KEYUP
            );
        }
        assert_unicode_pairs(&inputs[18..]);
    }
}
