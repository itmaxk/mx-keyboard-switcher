//! Input injection via `SendInput`. Every injected event carries `dwExtraInfo
//! == MAGIC` so the low-level hook can recognize and ignore it (clean, race-free
//! self-event suppression).
//!
//! Text is normally typed as `KEYEVENTF_UNICODE` (VK_PACKET) units, NOT as
//! physical scancodes. Scancodes are translated by the *receiving application* using
//! whatever keyboard layout it has applied at that moment, and apps with their
//! own layout handling (Qt apps like Telegram Desktop) apply
//! `WM_INPUTLANGCHANGE` asynchronously — a correction injected right after the
//! switch was retyped in the OLD layout ("Vbh" instead of "Мир"). Unicode
//! injection delivers the exact characters regardless of any layout, while the
//! system layout switch (see `layout.rs`) still happens so the user's *next*
//! keystrokes come out in the right language.
//!
//! Notepad++ corrupts Unicode packet replacement batches, so replacement only
//! for that exact process uses one fully preflighted physical-scancode batch.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use mxks_core::layout::{char_to_key, is_letter_of, to_lower, Lang};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, VIRTUAL_KEY,
    VK_BACK, VK_CAPITAL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_TAB,
};

use super::{keymap, MAGIC};

const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(2);
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(250);
const NOTEPAD_PLUS_PLUS: &str = "notepad++.exe";
const SC_SHIFT: u16 = 0x2A;

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

fn append_scan_pair(inputs: &mut Vec<INPUT>, scan: u16) {
    inputs.push(key_input(0, scan, KEYEVENTF_SCANCODE));
    inputs.push(key_input(0, scan, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP));
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

fn append_physical_char(inputs: &mut Vec<INPUT>, ch: char, caps_lock_on: bool) -> Option<()> {
    if ch == ' ' {
        append_scan_pair(inputs, keymap::SC_SPACE as u16);
        return Some(());
    }

    let lang = if is_letter_of(ch, Lang::En) {
        Lang::En
    } else if is_letter_of(ch, Lang::Ru) {
        Lang::Ru
    } else {
        return None;
    };
    let key = char_to_key(ch, lang)?;
    let scan = keymap::scan_of(key)?;
    let desired_uppercase = ch != to_lower(ch);
    let shifted = desired_uppercase ^ caps_lock_on;
    if shifted {
        inputs.push(key_input(0, SC_SHIFT, KEYEVENTF_SCANCODE));
    }
    append_scan_pair(inputs, scan);
    if shifted {
        inputs.push(key_input(0, SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP));
    }
    Some(())
}

/// Build the whole physical replacement before any event is sent. `None`
/// means at least one character cannot be represented safely and the caller
/// must fall back to the complete Unicode batch.
fn physical_replacement_inputs(
    erase: usize,
    text: &str,
    trailing: &str,
    caps_lock_on: bool,
) -> Option<Vec<INPUT>> {
    let mut inputs = Vec::with_capacity(
        erase
            .saturating_mul(2)
            .saturating_add((text.chars().count() + trailing.chars().count()).saturating_mul(4)),
    );
    for _ in 0..erase {
        append_scan_pair(&mut inputs, keymap::SC_BACKSPACE as u16);
    }
    for ch in text.chars().chain(trailing.chars()) {
        append_physical_char(&mut inputs, ch, caps_lock_on)?;
    }
    Some(inputs)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputMode {
    Unicode,
    PhysicalScancode,
}

impl InputMode {
    fn as_str(self) -> &'static str {
        match self {
            InputMode::Unicode => "unicode",
            InputMode::PhysicalScancode => "physical_scancode",
        }
    }
}

fn replacement_inputs_for_app(
    app: Option<&str>,
    erase: usize,
    text: &str,
    trailing: &str,
    caps_lock_on: bool,
) -> (Vec<INPUT>, InputMode) {
    if app == Some(NOTEPAD_PLUS_PLUS) {
        if let Some(inputs) = physical_replacement_inputs(erase, text, trailing, caps_lock_on) {
            return (inputs, InputMode::PhysicalScancode);
        }
    }
    (
        replacement_inputs(erase, text, trailing),
        InputMode::Unicode,
    )
}

fn caps_lock_is_on() -> bool {
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 }
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
        let app = super::foreground_process_name();
        let caps_lock_on = app.as_deref() == Some(NOTEPAD_PLUS_PLUS) && caps_lock_is_on();
        let (inputs, input_mode) =
            replacement_inputs_for_app(app.as_deref(), erase, text, trailing, caps_lock_on);
        tracing::info!(
            input_mode = input_mode.as_str(),
            app = app.as_deref().unwrap_or("<unknown>"),
            "SendInput replacement selected"
        );
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

    fn assert_replacement(inputs: &[INPUT], erase: usize, replacement: &str) {
        let delete_events = erase * 2;
        assert_eq!(
            inputs.len(),
            delete_events + replacement.encode_utf16().count() * 2
        );
        assert_tagged(inputs);

        for pair in inputs[..delete_events].chunks_exact(2) {
            let down = keyboard(&pair[0]);
            let up = keyboard(&pair[1]);
            assert_eq!(down.wVk, VK_BACK);
            assert_eq!(up.wVk, VK_BACK);
            assert_eq!(down.wScan, 0);
            assert_eq!(up.wScan, 0);
            assert_eq!(down.dwFlags, KEYBD_EVENT_FLAGS(0));
            assert_eq!(up.dwFlags, KEYEVENTF_KEYUP);
        }

        let unicode = &inputs[delete_events..];
        assert_unicode_pairs(unicode);
        let actual_units: Vec<_> = unicode
            .chunks_exact(2)
            .map(|pair| keyboard(&pair[0]).wScan)
            .collect();
        assert_eq!(actual_units, replacement.encode_utf16().collect::<Vec<_>>());
    }

    fn assert_physical_pairs(inputs: &[INPUT], expected_scans: &[u16]) {
        assert_eq!(inputs.len(), expected_scans.len() * 2);
        assert_tagged(inputs);
        for (pair, expected_scan) in inputs.chunks_exact(2).zip(expected_scans) {
            let down = keyboard(&pair[0]);
            let up = keyboard(&pair[1]);
            assert_eq!(down.wVk.0, 0);
            assert_eq!(up.wVk.0, 0);
            assert_eq!(down.wScan, *expected_scan);
            assert_eq!(up.wScan, *expected_scan);
            assert_eq!(down.dwFlags, KEYEVENTF_SCANCODE);
            assert_eq!(up.dwFlags, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP);
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
        // First hotkey press: erase `how`, then type `рщц ` in one SendInput
        // transaction (three Delete pairs, four Unicode pairs).
        let to_russian = replacement_inputs(3, "рщц", " ");
        assert_replacement(&to_russian, 3, "рщц ");

        // Toggle back: erase `рщц `, then type `how ` in one SendInput
        // transaction (four Delete pairs, four Unicode pairs).
        let to_english = replacement_inputs(4, "how", " ");
        assert_replacement(&to_english, 4, "how ");
    }

    #[test]
    fn notepad_rfr_to_kak_space_is_one_physical_batch() {
        let (inputs, mode) =
            replacement_inputs_for_app(Some(NOTEPAD_PLUS_PLUS), 3, "как", " ", false);
        assert_eq!(mode, InputMode::PhysicalScancode);
        let scans = [
            keymap::SC_BACKSPACE as u16,
            keymap::SC_BACKSPACE as u16,
            keymap::SC_BACKSPACE as u16,
            keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap(),
            keymap::scan_of(mxks_core::keycode::PhysKey::F).unwrap(),
            keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap(),
            keymap::SC_SPACE as u16,
        ];

        assert_eq!(inputs.len(), 14);
        assert_physical_pairs(&inputs, &scans);
    }

    #[test]
    fn notepad_kak_space_to_rfr_space_is_one_physical_batch() {
        let (inputs, mode) =
            replacement_inputs_for_app(Some(NOTEPAD_PLUS_PLUS), 4, "rfr", " ", false);
        assert_eq!(mode, InputMode::PhysicalScancode);
        let scans = [
            keymap::SC_BACKSPACE as u16,
            keymap::SC_BACKSPACE as u16,
            keymap::SC_BACKSPACE as u16,
            keymap::SC_BACKSPACE as u16,
            keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap(),
            keymap::scan_of(mxks_core::keycode::PhysKey::F).unwrap(),
            keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap(),
            keymap::SC_SPACE as u16,
        ];

        assert_eq!(inputs.len(), 16);
        assert_physical_pairs(&inputs, &scans);
    }

    #[test]
    fn uppercase_physical_character_is_wrapped_in_shift() {
        let inputs = physical_replacement_inputs(0, "Как", "", false).expect("physical batch");
        assert_tagged(&inputs);
        let r = keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap();
        let f = keymap::scan_of(mxks_core::keycode::PhysKey::F).unwrap();
        let actual: Vec<_> = inputs
            .iter()
            .map(|input| {
                let key = keyboard(input);
                assert_eq!(key.wVk.0, 0);
                (key.wScan, key.dwFlags)
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (f, KEYEVENTF_SCANCODE),
                (f, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            ]
        );
    }

    #[test]
    fn caps_lock_on_wraps_lowercase_physical_letters_in_shift() {
        let inputs = physical_replacement_inputs(0, "как", "", true).expect("physical batch");
        assert_tagged(&inputs);
        let r = keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap();
        let f = keymap::scan_of(mxks_core::keycode::PhysKey::F).unwrap();
        let actual: Vec<_> = inputs
            .iter()
            .map(|input| {
                let key = keyboard(input);
                assert_eq!(key.wVk.0, 0);
                (key.wScan, key.dwFlags)
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (f, KEYEVENTF_SCANCODE),
                (f, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            ]
        );
    }

    #[test]
    fn caps_lock_on_uses_shift_only_for_lowercase_remainder() {
        let inputs = physical_replacement_inputs(0, "Как", "", true).expect("physical batch");
        assert_tagged(&inputs);
        let r = keymap::scan_of(mxks_core::keycode::PhysKey::R).unwrap();
        let f = keymap::scan_of(mxks_core::keycode::PhysKey::F).unwrap();
        let actual: Vec<_> = inputs
            .iter()
            .map(|input| {
                let key = keyboard(input);
                assert_eq!(key.wVk.0, 0);
                (key.wScan, key.dwFlags)
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (f, KEYEVENTF_SCANCODE),
                (f, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE),
                (r, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
                (SC_SHIFT, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP),
            ]
        );
    }

    #[test]
    fn unsupported_notepad_character_falls_back_to_whole_unicode_batch() {
        assert!(physical_replacement_inputs(3, "как!", " ", false).is_none());

        let (inputs, mode) =
            replacement_inputs_for_app(Some(NOTEPAD_PLUS_PLUS), 3, "как!", " ", false);
        assert_eq!(mode, InputMode::Unicode);
        assert_replacement(&inputs, 3, "как! ");
    }

    #[test]
    fn physical_mode_is_limited_to_exact_notepad_process_name() {
        for app in [None, Some("chrome.exe"), Some("NOTEPAD++.EXE")] {
            let (inputs, mode) = replacement_inputs_for_app(app, 3, "как", " ", false);
            assert_eq!(mode, InputMode::Unicode);
            assert_replacement(&inputs, 3, "как ");
        }
    }
}
