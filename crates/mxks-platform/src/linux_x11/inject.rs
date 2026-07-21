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
//! thread drops its own echo. Each injected batch additionally runs inside a
//! [`Suppress`] injection window and ends with a server round-trip: when an
//! injection method returns, every fake event has been processed by the server
//! and its echo is already ordered into the RECORD stream, ahead of whatever
//! the user presses next. That ordering is what keeps the per-keycode echo
//! counters consuming the right events.

use anyhow::Result;
use mxks_core::layout::{char_to_key, is_letter_of, to_lower, Lang};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

use super::keymap::{self, KC_BACKSPACE, KC_SHIFT, KC_SPACE, KC_TAB};
use super::suppress::Suppress;

pub struct X11Injector {
    conn: RustConnection,
    suppress: Suppress,
    /// Every keycode bound to a modifier, released before each injection so a
    /// physically-held modifier (e.g. the Ctrl of a Ctrl+Pause hotkey) does not
    /// turn our injected keystrokes into Ctrl+/Alt+ chords and corrupt them.
    modifier_keycodes: Vec<u8>,
}

impl X11Injector {
    pub fn new(suppress: Suppress) -> Result<Self> {
        let (conn, _) = RustConnection::connect(None)?;
        let modifier_keycodes = conn
            .get_modifier_mapping()?
            .reply()
            .map(|m| m.keycodes.into_iter().filter(|&k| k != 0).collect())
            .unwrap_or_default();
        Ok(X11Injector {
            conn,
            suppress,
            modifier_keycodes,
        })
    }

    /// Run one injected batch: open the suppression window, send the fake
    /// events, then round-trip so the server has processed them all before we
    /// return (their echoes are then already ordered into the RECORD stream).
    fn injected<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let _guard = self.suppress.injection();
        let out = f()?;
        self.conn.flush()?;
        self.conn.get_input_focus()?.reply()?;
        Ok(out)
    }

    /// Fake-release every modifier key so injected taps are unmodified. A
    /// release of a key that is not held is a harmless no-op.
    fn clear_modifiers(&self) -> Result<()> {
        for &kc in &self.modifier_keycodes {
            self.fake_key(false, kc)?;
        }
        self.conn.flush()?;
        Ok(())
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
            // A correction is triggered by a real Space press, so that key may
            // still be physically held. Release it before the injected tap or
            // XTEST can treat our press as already down and emit no KeyPress.
            // This release has no matching press echo to suppress.
            self.fake_key(false, KC_SPACE)?;
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
        self.injected(|| {
            self.clear_modifiers()?;
            for _ in 0..n {
                self.tap_key(KC_BACKSPACE, false)?;
            }
            Ok(())
        })
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        self.injected(|| {
            self.clear_modifiers()?;
            for c in text.chars() {
                self.tap_char(c)?;
            }
            Ok(())
        })
    }

    /// One window + one round-trip for the whole erase-and-retype sequence, so
    /// a correction is a single atomic batch from the suppressor's viewpoint.
    fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
        self.injected(|| {
            self.clear_modifiers()?;
            for _ in 0..erase {
                self.tap_key(KC_BACKSPACE, false)?;
            }
            for c in text.chars().chain(trailing.chars()) {
                self.tap_char(c)?;
            }
            Ok(())
        })
    }

    fn tab(&mut self) -> Result<()> {
        self.injected(|| {
            self.clear_modifiers()?;
            self.tap_key(KC_TAB, false)
        })
    }
}
