//! XTEST-based input injection.
//!
//! Backspaces are sent as the real Backspace keycode. Arbitrary text is typed
//! by temporarily binding a *spare* keycode to each character's Unicode keysym
//! (the xdotool technique), which is independent of the active layout/group and
//! types exact Unicode. All levels of the spare key are set to the same keysym
//! so it produces the character regardless of the current group or Shift state.

use anyhow::Result;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

use super::keymap::KC_BACKSPACE;
use super::suppress::Suppress;

/// X11 Unicode keysym base (keysym = 0x0100_0000 + codepoint).
const UNICODE_KEYSYM_BASE: u32 = 0x0100_0000;

pub struct X11Injector {
    conn: RustConnection,
    suppress: Suppress,
    spare: u8,
    per: u8,
}

impl X11Injector {
    pub fn new(suppress: Suppress, spare: u8, keysyms_per_keycode: u8) -> Result<Self> {
        let (conn, _) = RustConnection::connect(None)?;
        Ok(X11Injector {
            conn,
            suppress,
            spare,
            per: keysyms_per_keycode.max(2),
        })
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

    fn tap(&self, keycode: u8) -> Result<()> {
        self.fake_key(true, keycode)?;
        self.fake_key(false, keycode)?;
        Ok(())
    }

    fn bind_spare(&self, keysym: u32) -> Result<()> {
        let row = vec![keysym; self.per as usize];
        // .check() round-trips, guaranteeing the server applied the mapping
        // before we synthesize the key press that depends on it.
        self.conn
            .change_keyboard_mapping(1, self.spare, self.per, &row)?
            .check()?;
        Ok(())
    }

    fn unbind_spare(&self) -> Result<()> {
        let row = vec![NONE; self.per as usize];
        self.conn
            .change_keyboard_mapping(1, self.spare, self.per, &row)?;
        self.conn.flush()?;
        Ok(())
    }
}

impl crate::KeyInjector for X11Injector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        if n == 0 {
            return Ok(());
        }
        self.suppress.expect_backspaces(n);
        for _ in 0..n {
            self.tap(KC_BACKSPACE)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            let keysym = UNICODE_KEYSYM_BASE + c as u32;
            self.bind_spare(keysym)?;
            self.tap(self.spare)?;
            self.conn.flush()?;
        }
        self.unbind_spare()?;
        Ok(())
    }
}
