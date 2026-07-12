//! Dynamic accept-key interception via `XGrabKey`.
//!
//! XRecord is a passive observer and cannot block a key from reaching the
//! focused application, so the accept key (Tab by default) is grabbed on the
//! root window — but only while a suggestion is visible. A grabbed key is
//! delivered exclusively to this client, which *is* the swallow.
//!
//! The grab is issued for the exact modifier combination of the accept spec
//! (plus CapsLock/NumLock variants), never `AnyModifier`, so chords like
//! Alt+Tab or Ctrl+Tab keep working while a suggestion is shown.
//!
//! Known benign race: between "suggestion shown" and the grab taking effect a
//! very fast accept-key press reaches the app normally; conversely a stale
//! `Accept` (suggestion already dismissed) is handled by the engine, which
//! re-injects a real key so the keystroke is not lost.

use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{RecvTimeoutError, Sender};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::keymap;
use crate::event::{KeyEvent, KeyKind};
use crate::{InterceptCmd, InterceptControl};
use mxks_core::hotkey::HotkeySpec;

// X11 modifier mask bits (same values record.rs uses).
const SHIFT: u16 = 1 << 0;
const LOCK: u16 = 1 << 1; // CapsLock
const CONTROL: u16 = 1 << 2;
const ALT: u16 = 1 << 3; // Mod1
const NUMLOCK: u16 = 1 << 4; // Mod2
const SUPER: u16 = 1 << 6; // Mod4

/// CapsLock/NumLock variants added to every grab so the accept key works
/// regardless of lock state.
const LOCK_VARIANTS: [u16; 4] = [0, LOCK, NUMLOCK, LOCK | NUMLOCK];

/// Spawn the interception thread. It idles on the command channel and only
/// polls X while a grab is active.
pub fn spawn(control: InterceptControl, tx: Sender<KeyEvent>) {
    std::thread::Builder::new()
        .name("mxks-intercept".into())
        .spawn(move || {
            if let Err(e) = run(control, tx) {
                tracing::warn!("accept-key interception unavailable: {e:#}");
            }
        })
        .expect("spawn intercept thread");
}

fn base_mask(spec: &HotkeySpec) -> u16 {
    let mut m = 0u16;
    if spec.shift {
        m |= SHIFT;
    }
    if spec.ctrl {
        m |= CONTROL;
    }
    if spec.alt {
        m |= ALT;
    }
    if spec.meta {
        m |= SUPER;
    }
    m
}

/// Resolve the accept spec's key name to an X11 keycode: letters via the
/// physical key, named keys by scanning the keyboard mapping for a keysym
/// whose canonical name matches.
fn resolve_keycode(conn: &RustConnection, key: &str) -> Option<u8> {
    if key.len() == 1 {
        let c = key.chars().next()?.to_ascii_lowercase();
        let phys = mxks_core::layout::char_to_key(c, mxks_core::layout::Lang::En)?;
        return Some(keymap::keycode_of(phys));
    }
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode - min + 1;
    let map = conn.get_keyboard_mapping(min, count).ok()?.reply().ok()?;
    let per = map.keysyms_per_keycode as usize;
    if per == 0 {
        return None;
    }
    for (i, chunk) in map.keysyms.chunks(per).enumerate() {
        for &sym in chunk {
            if keymap::named_keysym(sym).is_some_and(|n| n.eq_ignore_ascii_case(key)) {
                return Some(min + i as u8);
            }
        }
    }
    None
}

struct Grab {
    keycode: u8,
    mask: u16,
}

fn grab(conn: &RustConnection, root: u32, spec: &HotkeySpec) -> Option<Grab> {
    let Some(keycode) = resolve_keycode(conn, &spec.key) else {
        tracing::warn!("accept key {:?} has no keycode; not intercepting", spec.key);
        return None;
    };
    let mask = base_mask(spec);
    let mut ok = false;
    for lock in LOCK_VARIANTS {
        match conn.grab_key(
            false,
            root,
            ModMask::from(mask | lock),
            keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        ) {
            Ok(cookie) => match cookie.check() {
                Ok(()) => ok = true,
                // Typically BadAccess: another client owns this grab.
                Err(e) => tracing::debug!("grab_key variant failed: {e}"),
            },
            Err(e) => tracing::debug!("grab_key request failed: {e}"),
        }
    }
    let _ = conn.flush();
    if ok {
        Some(Grab { keycode, mask })
    } else {
        tracing::warn!("could not grab accept key (owned by another client?)");
        None
    }
}

fn ungrab(conn: &RustConnection, root: u32, g: &Grab) {
    for lock in LOCK_VARIANTS {
        let _ = conn.ungrab_key(g.keycode, root, ModMask::from(g.mask | lock));
    }
    let _ = conn.flush();
}

fn run(control: InterceptControl, tx: Sender<KeyEvent>) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None).context("intercept connection")?;
    let root = conn.setup().roots[screen_num].root;
    let cmds = control.commands().clone();
    let mut current: Option<Grab> = None;

    loop {
        let cmd = if current.is_none() {
            // Idle: no grab installed, block until told otherwise.
            match cmds.recv() {
                Ok(c) => Some(c),
                Err(_) => break,
            }
        } else {
            // Active: drain grabbed key events, then wait briefly for commands.
            while let Some(ev) = conn.poll_for_event()? {
                if let Event::KeyPress(_) = ev {
                    let _ = tx.send(KeyEvent {
                        kind: KeyKind::Accept,
                        down: true,
                        injected: false,
                    });
                }
            }
            match cmds.recv_timeout(Duration::from_millis(5)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };

        match cmd {
            Some(InterceptCmd::Active(true)) => {
                if current.is_none() {
                    current = grab(&conn, root, &control.current());
                }
            }
            Some(InterceptCmd::Active(false)) => {
                if let Some(g) = current.take() {
                    ungrab(&conn, root, &g);
                }
            }
            Some(InterceptCmd::Spec(spec)) => {
                if let Some(g) = current.take() {
                    ungrab(&conn, root, &g);
                    current = grab(&conn, root, &spec);
                }
            }
            None => {}
        }
    }

    if let Some(g) = current.take() {
        ungrab(&conn, root, &g);
    }
    Ok(())
}
