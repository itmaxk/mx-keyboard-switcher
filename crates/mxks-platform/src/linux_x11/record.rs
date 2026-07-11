//! Global key capture via the X11 RECORD extension.
//!
//! Two connections are used per the RECORD spec: one to control the context,
//! one to stream data. The stream is a blocking iterator, so this owns the
//! capture thread. Each 32-byte core event is parsed for its keycode and
//! modifier state; injected events are filtered out via [`Suppress`].
//!
//! The conversion hotkey is matched by *key name* (resolved from the keycode:
//! letters via the physical key, named keys via their keysym), which is robust
//! across keymaps — unlike hard-coded keycodes, where e.g. Pause is 110 on one
//! machine and 127 on another. The same resolution powers "press a key to
//! assign" capture mode.

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use x11rb::connection::Connection;
use x11rb::protocol::record::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::rust_connection::RustConnection;

use super::keymap::{self, KC_BACKSPACE};
use super::suppress::Suppress;
use crate::event::{KeyEvent, KeyKind};
use crate::HotkeyControl;
use mxks_core::hotkey::HotkeySpec;

/// RECORD reply category for data coming from the server.
const FROM_SERVER: u8 = 0;

// X11 keybutton modifier mask bits.
const SHIFT: u16 = 1 << 0;
const CONTROL: u16 = 1 << 2;
const ALT: u16 = 1 << 3; // Mod1
const SUPER: u16 = 1 << 6; // Mod4

pub struct X11Capture {
    suppress: Suppress,
    hotkey: HotkeyControl,
    /// Cached keymap for keycode → keysym resolution (named keys only; stable).
    keysyms: Vec<u32>,
    per: usize,
    min_keycode: u8,
}

struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
}

impl X11Capture {
    pub fn new(suppress: Suppress, hotkey: HotkeyControl) -> Self {
        X11Capture {
            suppress,
            hotkey,
            keysyms: Vec::new(),
            per: 0,
            min_keycode: 8,
        }
    }

    fn load_keymap(&mut self) -> Result<()> {
        let (conn, _) = RustConnection::connect(None)?;
        let setup = conn.setup();
        self.min_keycode = setup.min_keycode;
        let count = setup.max_keycode - setup.min_keycode + 1;
        let map = conn
            .get_keyboard_mapping(setup.min_keycode, count)?
            .reply()?;
        self.per = map.keysyms_per_keycode as usize;
        self.keysyms = map.keysyms;
        Ok(())
    }

    /// Canonical hotkey name for a keycode: letters via the physical key,
    /// otherwise a recognized named keysym.
    fn name_of(&self, keycode: u8) -> Option<String> {
        if let Some(name) = keymap::key_letter_name(keycode) {
            return Some(name);
        }
        if self.per > 0 && keycode >= self.min_keycode {
            let base = (keycode - self.min_keycode) as usize * self.per;
            for off in 0..self.per {
                if let Some(&sym) = self.keysyms.get(base + off) {
                    if let Some(name) = keymap::named_keysym(sym) {
                        return Some(name.to_string());
                    }
                }
            }
        }
        None
    }

    fn mods(state: u16) -> Mods {
        Mods {
            shift: state & SHIFT != 0,
            ctrl: state & CONTROL != 0,
            alt: state & ALT != 0,
            meta: state & SUPER != 0,
        }
    }

    /// Build a HotkeySpec for a captured key, if it is a sensible hotkey (a
    /// named key, or any key combined with a modifier — never a bare letter).
    fn capture_spec(&self, keycode: u8, m: &Mods) -> Option<HotkeySpec> {
        let name = self.name_of(keycode)?;
        let has_mod = m.ctrl || m.alt || m.meta;
        let is_bare_letter = name.len() == 1 && !has_mod;
        if is_bare_letter {
            return None; // avoid footgun: a plain letter as the hotkey
        }
        Some(HotkeySpec {
            ctrl: m.ctrl,
            shift: m.shift,
            alt: m.alt,
            meta: m.meta,
            key: name,
        })
    }

    fn matches_hotkey(&self, keycode: u8, m: &Mods) -> bool {
        let spec = self.hotkey.current();
        match self.name_of(keycode) {
            Some(name) => {
                name.eq_ignore_ascii_case(&spec.key)
                    && m.ctrl == spec.ctrl
                    && m.shift == spec.shift
                    && m.alt == spec.alt
                    && m.meta == spec.meta
            }
            None => false,
        }
    }

    fn classify(&self, keycode: u8, state: u16) -> Option<KeyKind> {
        let m = Self::mods(state);

        if self.matches_hotkey(keycode, &m) {
            return Some(KeyKind::Hotkey);
        }

        if m.ctrl || m.alt || m.meta {
            return Some(KeyKind::Reset);
        }
        if let Some(key) = keymap::phys_of(keycode) {
            return Some(KeyKind::Letter {
                key,
                shift: m.shift,
            });
        }
        if keycode == KC_BACKSPACE {
            return Some(KeyKind::Backspace);
        }
        if keycode == keymap::KC_SPACE {
            return Some(KeyKind::Boundary { sep: Some(' ') });
        }
        if keymap::is_boundary(keycode) {
            return Some(KeyKind::Boundary { sep: None });
        }
        Some(KeyKind::Reset)
    }

    /// Handle one 32-byte core event slice; forward an engine event if relevant.
    fn on_event(&self, data: &[u8], tx: &Sender<KeyEvent>) -> Result<()> {
        let response_type = data[0] & 0x7f;
        if response_type != KEY_PRESS_EVENT {
            let _ = KEY_RELEASE_EVENT; // we act only on key-down
            return Ok(());
        }
        let keycode = data[1];
        let state = u16::from_le_bytes([data[28], data[29]]);

        if self.suppress.should_drop(keycode) {
            return Ok(());
        }

        // Capture mode: record the next sensible key as the new hotkey.
        if self.hotkey.is_capturing() {
            let m = Self::mods(state);
            if let Some(spec) = self.capture_spec(keycode, &m) {
                self.hotkey.record(spec);
            }
            return Ok(()); // swallow while capturing
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
        self.load_keymap()
            .context("loading keymap for hotkey names")?;

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
