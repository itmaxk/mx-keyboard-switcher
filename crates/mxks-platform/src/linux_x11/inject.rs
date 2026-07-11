//! XTEST-based input injection by *replaying physical keycodes*.
//!
//! The corrector switches the XKB group to the target language before calling
//! `type_text`, so we can simply tap the physical key that produces each target
//! character in the now-active group (plus Shift for uppercase). This avoids any
//! runtime keymap mutation and the `MappingNotify` race that the previous
//! spare-keycode/Unicode-remap approach suffered from (it silently dropped or
//! corrupted characters).
//!
//! Every injected key-press is registered with [`Suppress`] so the capture
//! thread drops its own echo.

use anyhow::Result;
use mxks_core::layout::{char_to_key, is_letter_of, to_lower, Lang};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

use super::keymap::{self, KC_BACKSPACE, KC_SHIFT, KC_SPACE};
use super::suppress::Suppress;

pub struct X11Injector {
    conn: RustConnection,
    suppress: Suppress,
}

impl X11Injector {
    pub fn new(suppress: Suppress) -> Result<Self> {
        let (conn, _) = RustConnection::connect(None)?;
        Ok(X11Injector { conn, suppress })
    }

    fn fake_key(&self, press: bool, keycode: u8) -> Result<()> {
        let ty = if press {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.conn.xtest_fake_input(ty, keycode, 0, NONE, 0, 0, 0)?;
        Ok(())
    }

    /// Tap `keycode`, optionally with Shift held. Registers each key-press for
    /// echo suppression.
    fn tap_key(&self, keycode: u8, shift: bool) -> Result<()> {
        if shift {
            self.suppress.expect(KC_SHIFT);
            self.fake_key(true, KC_SHIFT)?;
        }
        self.suppress.expect(keycode);
        self.fake_key(true, keycode)?;
        self.fake_key(false, keycode)?;
        if shift {
            self.fake_key(false, KC_SHIFT)?;
        }
        Ok(())
    }

    /// Tap the physical key that produces `c` in the currently active group.
    fn tap_char(&self, c: char) -> Result<()> {
        if c == ' ' {
            return self.tap_key(KC_SPACE, false);
        }
        // Determine which layout treats `c` as a letter, then find its key.
        let lang = if is_letter_of(c, Lang::Ru) {
            Lang::Ru
        } else {
            Lang::En
        };
        let lower = to_lower(c);
        if let Some(key) = char_to_key(lower, lang) {
            let keycode = keymap::keycode_of(key);
            self.tap_key(keycode, c != lower)?;
        } else {
            tracing::warn!("cannot inject character {c:?}; skipping");
        }
        Ok(())
    }
}

impl crate::KeyInjector for X11Injector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.tap_key(KC_BACKSPACE, false)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            self.tap_char(c)?;
        }
        self.conn.flush()?;
        Ok(())
    }
}
