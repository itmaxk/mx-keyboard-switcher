//! Input injection via `SendInput`. Every injected event carries `dwExtraInfo
//! == MAGIC` so the low-level hook can recognize and ignore it (clean, race-free
//! self-event suppression).

use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VIRTUAL_KEY, VK_BACK,
};

use super::MAGIC;

pub struct WinInjector;

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(flags),
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

impl crate::KeyInjector for WinInjector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        let mut inputs = Vec::with_capacity(n * 2);
        for _ in 0..n {
            inputs.push(key_input(VK_BACK.0, 0, 0));
            inputs.push(key_input(VK_BACK.0, 0, KEYEVENTF_KEYUP.0));
        }
        if !inputs.is_empty() {
            send(&inputs)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        let mut inputs = Vec::new();
        for unit in text.encode_utf16() {
            inputs.push(key_input(0, unit, KEYEVENTF_UNICODE.0));
            inputs.push(key_input(0, unit, KEYEVENTF_UNICODE.0 | KEYEVENTF_KEYUP.0));
        }
        if !inputs.is_empty() {
            send(&inputs)?;
        }
        Ok(())
    }
}
