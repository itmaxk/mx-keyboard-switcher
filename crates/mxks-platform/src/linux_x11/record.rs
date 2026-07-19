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
use x11rb::protocol::xproto::{
    ConnectionExt as _, BUTTON_PRESS_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::rust_connection::RustConnection;

use super::keymap::{self, KC_BACKSPACE};
use super::suppress::{Filter, Suppress};
use crate::event::{KeyEvent, KeyKind};
use crate::{HotkeyControl, InterceptControl};
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
    intercept: InterceptControl,
    /// Cached keymap for keycode → keysym resolution (named keys only; stable).
    keysyms: Vec<u32>,
    per: usize,
    min_keycode: u8,
}

#[derive(Clone, Copy)]
struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
}

impl X11Capture {
    pub fn new(suppress: Suppress, hotkey: HotkeyControl, intercept: InterceptControl) -> Self {
        X11Capture {
            suppress,
            hotkey,
            intercept,
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

    /// True if `keycode` is a modifier key (Ctrl/Shift/Alt/Super/…).
    fn is_modifier(&self, keycode: u8) -> bool {
        if self.per == 0 || keycode < self.min_keycode {
            return false;
        }
        let base = (keycode - self.min_keycode) as usize * self.per;
        (0..self.per).any(|off| {
            self.keysyms
                .get(base + off)
                .is_some_and(|&s| keymap::is_modifier_keysym(s))
        })
    }

    /// Normalize the modifier state for a resolved key name. The Pause/Break key
    /// emits a phantom Control on many keyboards (its scancode contains a Ctrl
    /// prefix), so we ignore Control for it — otherwise plain "Pause" never
    /// matches.
    fn norm_mods(name: &Option<String>, m: &Mods) -> Mods {
        let mut m = *m;
        if matches!(name.as_deref(), Some("PAUSE")) {
            m.ctrl = false;
        }
        m
    }

    /// Build a HotkeySpec for a captured key, if it is a sensible hotkey (a
    /// named key, or any key combined with a modifier — never a bare letter).
    fn capture_spec(&self, name: &Option<String>, m: &Mods) -> Option<HotkeySpec> {
        let name = name.clone()?;
        let m = Self::norm_mods(&Some(name.clone()), m);
        let has_mod = m.ctrl || m.alt || m.meta;
        if name.len() == 1 && !has_mod {
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

    fn matches_spec(spec: &HotkeySpec, name: &Option<String>, m: &Mods) -> bool {
        match name {
            Some(name) => {
                // Pause carries a phantom Control on many keyboards, so don't
                // compare Control for it (match "Pause" and "Ctrl+Pause" alike).
                let ignore_ctrl = name.eq_ignore_ascii_case("PAUSE");
                name.eq_ignore_ascii_case(&spec.key)
                    && (ignore_ctrl || m.ctrl == spec.ctrl)
                    && m.shift == spec.shift
                    && m.alt == spec.alt
                    && m.meta == spec.meta
            }
            None => false,
        }
    }

    fn matches_hotkey(&self, name: &Option<String>, m: &Mods) -> bool {
        Self::matches_spec(&self.hotkey.current(), name, m)
    }

    fn classify(&self, keycode: u8, state: u16) -> Option<KeyKind> {
        let m = Self::mods(state);
        let name = self.name_of(keycode);

        if self.matches_hotkey(&name, &m) {
            return Some(KeyKind::Hotkey);
        }
        // Bare modifier keys (incl. Pause's phantom Ctrl) must not reset the buffer.
        if self.is_modifier(keycode) {
            return None;
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
        if response_type == BUTTON_PRESS_EVENT {
            // A click (buttons 1-3) moves the caret or the focus, so the
            // in-progress word no longer matches what is on screen. Wheel
            // scrolling (4/5, incl. horizontal 6/7) leaves the caret alone.
            let button = data[1];
            if (1..=3).contains(&button) {
                let _ = tx.send(KeyEvent {
                    kind: KeyKind::Reset,
                    down: true,
                    injected: false,
                });
            }
            return Ok(());
        }
        if response_type != KEY_PRESS_EVENT {
            let _ = KEY_RELEASE_EVENT; // we act only on key-down
            return Ok(());
        }
        let keycode = data[1];
        let state = u16::from_le_bytes([data[28], data[29]]);

        match self.suppress.filter(keycode) {
            Filter::DropEcho => return Ok(()),
            Filter::DropInjecting => {
                tracing::trace!("dropping keycode {keycode} pressed during injection");
                return Ok(());
            }
            Filter::Real => {}
        }

        // While the accept key is grabbed (suggestion visible), XRecord still
        // records the grabbed press; the intercept thread already delivered it
        // as `Accept`, so drop it here to avoid double handling.
        if self.intercept.is_active() {
            let m = Self::mods(state);
            let name = self.name_of(keycode);
            if Self::matches_spec(&self.intercept.current(), &name, &m) {
                return Ok(());
            }
        }

        // Capture mode: record the next sensible key as the new hotkey.
        if self.hotkey.is_capturing() {
            let m = Self::mods(state);
            let name = self.name_of(keycode);
            if let Some(spec) = self.capture_spec(&name, &m) {
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

        // Accept-key interception (XGrabKey) runs on its own thread; it idles
        // until the engine activates it for a visible suggestion.
        super::intercept::spawn(self.intercept.clone(), tx.clone());

        // Focus watcher: resets the word buffer when the active window changes.
        super::focus_watch::spawn(tx.clone());

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
                last: BUTTON_PRESS_EVENT, // KeyPress, KeyRelease, ButtonPress
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
