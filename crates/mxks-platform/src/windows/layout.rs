//! Reading and switching the active keyboard layout via Win32.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use mxks_core::layout::Lang;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, LoadKeyboardLayoutW, ACTIVATE_KEYBOARD_LAYOUT_FLAGS, KLF_ACTIVATE,
    KLF_SUBSTITUTE_OK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, PostMessageW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    WM_INPUTLANGCHANGEREQUEST,
};

const LANGID_EN: u16 = 0x0409;
const LANGID_RU: u16 = 0x0419;
const SWITCH_TIMEOUT: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(1);
const NOTEPAD_PLUS_PLUS: &str = "notepad++.exe";

pub struct WinLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivationPollFailure {
    ForegroundChanged,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryMode {
    Posted,
    Synchronous,
}

impl DeliveryMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Posted => "posted",
            Self::Synchronous => "synchronous",
        }
    }
}

fn delivery_mode_for_app(app: Option<&str>) -> DeliveryMode {
    if app == Some(NOTEPAD_PLUS_PLUS) {
        DeliveryMode::Synchronous
    } else {
        DeliveryMode::Posted
    }
}

/// Poll layout activation through injectable observations and waiting. Keeping
/// the decision loop independent of Win32 and wall-clock time makes delayed
/// message processing and focus races deterministic in tests.
fn poll_until_active(
    target_langid: u16,
    mut observe: impl FnMut() -> (bool, u16),
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> std::result::Result<(), ActivationPollFailure> {
    loop {
        let (foreground_stable, current_langid) = observe();
        if !foreground_stable {
            return Err(ActivationPollFailure::ForegroundChanged);
        }
        if current_langid == target_langid {
            return Ok(());
        }
        if elapsed() >= SWITCH_TIMEOUT {
            return Err(ActivationPollFailure::Exhausted);
        }
        sleep(POLL_INTERVAL);
    }
}

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

        let app = super::foreground_process_name();
        let delivery_mode = delivery_mode_for_app(app.as_deref());
        tracing::info!(
            delivery_mode = delivery_mode.as_str(),
            app = app.as_deref().unwrap_or("<unknown>"),
            "keyboard layout change delivery selected"
        );
        match delivery_mode {
            DeliveryMode::Posted => unsafe {
                PostMessageW(
                    Some(keyboard.target_hwnd),
                    WM_INPUTLANGCHANGEREQUEST,
                    WPARAM(0),
                    LPARAM(hkl.0 as isize),
                )
            }
            .context("could not queue keyboard layout change for focused window")?,
            DeliveryMode::Synchronous => {
                let delivered = unsafe {
                    SendMessageTimeoutW(
                        keyboard.target_hwnd,
                        WM_INPUTLANGCHANGEREQUEST,
                        WPARAM(0),
                        LPARAM(hkl.0 as isize),
                        SMTO_ABORTIFHUNG | SMTO_BLOCK,
                        SWITCH_TIMEOUT.as_millis() as u32,
                        None,
                    )
                };
                if delivered.0 == 0 {
                    bail!("synchronous keyboard layout change delivery failed or timed out");
                }
            }
        }

        let started = Instant::now();
        let result = poll_until_active(
            target_langid,
            || {
                let foreground_stable =
                    unsafe { GetForegroundWindow() } == keyboard.foreground_hwnd;
                let current = unsafe { GetKeyboardLayout(keyboard.thread_id) };
                (foreground_stable, langid(current))
            },
            || started.elapsed(),
            std::thread::sleep,
        );
        match result {
            Ok(()) => Ok(()),
            Err(ActivationPollFailure::ForegroundChanged) => {
                bail!("foreground window changed while activating keyboard layout")
            }
            Err(ActivationPollFailure::Exhausted) => {
                bail!("target keyboard layout did not activate before deadline")
            }
        }
    }
}

fn langid(hkl: windows::Win32::UI::Input::KeyboardAndMouse::HKL) -> u16 {
    (hkl.0 as usize & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn only_exact_notepad_process_uses_synchronous_delivery() {
        assert_eq!(
            delivery_mode_for_app(Some("notepad++.exe")),
            DeliveryMode::Synchronous
        );
        for app in [
            None,
            Some("NOTEPAD++.EXE"),
            Some("telegram.exe"),
            Some("chrome.exe"),
        ] {
            assert_eq!(delivery_mode_for_app(app), DeliveryMode::Posted);
        }
    }

    #[test]
    fn delayed_activation_after_many_polls_succeeds() {
        let mut polls = 0;
        let elapsed = Cell::new(Duration::ZERO);
        let result = poll_until_active(
            LANGID_RU,
            || {
                polls += 1;
                let current = if polls >= 26 { LANGID_RU } else { LANGID_EN };
                (true, current)
            },
            || elapsed.get(),
            |duration| elapsed.set(elapsed.get() + duration),
        );

        assert_eq!(result, Ok(()));
        assert!(polls >= 25);
    }

    #[test]
    fn foreground_change_fails_polling() {
        let mut polls = 0;
        let elapsed = Cell::new(Duration::ZERO);
        let result = poll_until_active(
            LANGID_RU,
            || {
                polls += 1;
                (polls < 4, LANGID_EN)
            },
            || elapsed.get(),
            |duration| elapsed.set(elapsed.get() + duration),
        );

        assert_eq!(result, Err(ActivationPollFailure::ForegroundChanged));
        assert_eq!(polls, 4);
    }

    #[test]
    fn exhausted_polling_fails() {
        let mut polls = 0;
        let elapsed = Cell::new(Duration::ZERO);
        let result = poll_until_active(
            LANGID_RU,
            || {
                polls += 1;
                (true, LANGID_EN)
            },
            || elapsed.get(),
            |duration| elapsed.set(elapsed.get() + duration),
        );

        assert_eq!(result, Err(ActivationPollFailure::Exhausted));
        assert!(polls > 25);
    }
}
