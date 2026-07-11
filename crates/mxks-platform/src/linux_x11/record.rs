//! Global key capture via the X11 RECORD extension.
//!
//! Two connections are used per the RECORD spec: one to control the context,
//! one to stream data. The stream is a blocking iterator, so this owns the
//! capture thread. Each 32-byte core event is parsed for its keycode and
//! modifier state; injected events are filtered out via [`Suppress`].

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use mxks_core::hotkey::HotkeySpec;
use x11rb::connection::Connection;
use x11rb::protocol::record::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::rust_connection::RustConnection;

use super::keymap::{self, KC_BACKSPACE};
use super::suppress::Suppress;
use crate::event::{KeyEvent, KeyKind};

/// RECORD reply category for data coming from the server.
const FROM_SERVER: u8 = 0;

// X11 keybutton modifier mask bits.
const SHIFT: u16 = 1 << 0;
const CONTROL: u16 = 1 << 2;
const ALT: u16 = 1 << 3; // Mod1
const SUPER: u16 = 1 << 6; // Mod4

pub struct X11Capture {
    suppress: Suppress,
    hotkey_keycode: Option<u8>,
    hotkey: HotkeySpec,
}

impl X11Capture {
    pub fn new(suppress: Suppress, hotkey: HotkeySpec) -> Self {
        let hotkey_keycode = keymap::keycode_for_name(&hotkey.key);
        X11Capture {
            suppress,
            hotkey_keycode,
            hotkey,
        }
    }

    fn classify(&self, keycode: u8, state: u16) -> Option<KeyKind> {
        let shift = state & SHIFT != 0;
        let ctrl = state & CONTROL != 0;
        let alt = state & ALT != 0;
        let meta = state & SUPER != 0;

        // Hotkey has priority over everything else.
        if Some(keycode) == self.hotkey_keycode
            && ctrl == self.hotkey.ctrl
            && shift == self.hotkey.shift
            && alt == self.hotkey.alt
            && meta == self.hotkey.meta
        {
            return Some(KeyKind::Hotkey);
        }

        // Any control/alt/super chord invalidates the word buffer.
        if ctrl || alt || meta {
            return Some(KeyKind::Reset);
        }

        if let Some(key) = keymap::phys_of(keycode) {
            return Some(KeyKind::Letter { key, shift });
        }
        if keycode == KC_BACKSPACE {
            return Some(KeyKind::Backspace);
        }
        if keycode == keymap::KC_SPACE {
            // Only Space triggers autocorrection in v1; re-typed after a fix.
            return Some(KeyKind::Boundary { sep: Some(' ') });
        }
        if keymap::is_boundary(keycode) {
            return Some(KeyKind::Boundary { sep: None });
        }
        // Esc, arrows, function keys, etc.
        Some(KeyKind::Reset)
    }

    /// Handle one 32-byte core event slice; forward an engine event if relevant.
    fn on_event(&self, data: &[u8], tx: &Sender<KeyEvent>) -> Result<()> {
        let response_type = data[0] & 0x7f;
        if response_type != KEY_PRESS_EVENT {
            // We act only on key-down; ignore key-up (KEY_RELEASE_EVENT).
            let _ = KEY_RELEASE_EVENT;
            return Ok(());
        }
        let keycode = data[1];
        let state = u16::from_le_bytes([data[28], data[29]]);

        if self.suppress.should_drop(keycode) {
            return Ok(());
        }

        if let Some(kind) = self.classify(keycode, state) {
            let _ = tx.send(KeyEvent {
                kind,
                down: true,
                injected: false,
            });
        }
        Ok(())
    }
}

impl crate::KeyCapture for X11Capture {
    fn run(&mut self, tx: Sender<KeyEvent>) -> Result<()> {
        let (ctrl_conn, _) = RustConnection::connect(None).context("RECORD control connection")?;
        let (data_conn, _) = RustConnection::connect(None).context("RECORD data connection")?;

        let rc = ctrl_conn.generate_id()?;
        let empty8 = record::Range8 { first: 0, last: 0 };
        let empty_ext = record::ExtRange {
            major: record::Range8 { first: 0, last: 0 },
            minor: record::Range16 { first: 0, last: 0 },
        };
        let range = record::Range {
            core_requests: empty8,
            core_replies: empty8,
            ext_requests: empty_ext,
            ext_replies: empty_ext,
            delivered_events: empty8,
            device_events: record::Range8 {
                first: KEY_PRESS_EVENT,
                last: KEY_RELEASE_EVENT,
            },
            errors: empty8,
            client_started: false,
            client_died: false,
        };
        ctrl_conn
            .record_create_context(rc, 0, &[record::CS::ALL_CLIENTS.into()], &[range])?
            .check()
            .context("record_create_context")?;

        for reply in data_conn.record_enable_context(rc)? {
            let reply = reply?;
            if reply.category != FROM_SERVER {
                continue;
            }
            let mut data = &reply.data[..];
            while data.len() >= 32 {
                self.on_event(&data[..32], &tx)?;
                data = &data[32..];
            }
        }
        Ok(())
    }
}
