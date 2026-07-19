//! Resets the engine's word state when the active window changes.
//!
//! Subscribes to `PropertyNotify` on the root window and watches
//! `_NET_ACTIVE_WINDOW`. On a real change of the active window id it feeds a
//! `KeyKind::Reset` into the engine channel — the same event a click or an
//! arrow key produces — so the in-progress word, the last-word hotkey state,
//! and any visible suggestion never leak from one application into another.
//!
//! Window managers rewrite the property liberally, so the watcher dedupes on
//! the window id. Injected corrections never touch `_NET_ACTIVE_WINDOW`, so
//! they cannot trigger a self-reset. If setup fails (e.g. a WM without EWMH),
//! the watcher just logs and exits: button-press resets from the RECORD stream
//! remain as the fallback.

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::event::{KeyEvent, KeyKind};

pub fn spawn(tx: Sender<KeyEvent>) {
    std::thread::Builder::new()
        .name("mxks-focus-watch".into())
        .spawn(move || {
            if let Err(e) = run(tx) {
                tracing::warn!(
                    "focus watcher stopped: {e:#}; window switches will not reset the word buffer"
                );
            }
        })
        .expect("spawn focus watcher thread");
}

fn run(tx: Sender<KeyEvent>) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None).context("focus watcher connection")?;
    let root = conn.setup().roots[screen_num].root;
    let net_active_window = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
        .reply()?
        .atom;
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?
    .check()
    .context("subscribing to root PropertyNotify")?;

    // Seed with the current active window so startup produces no spurious Reset.
    let mut last = active_window(&conn, root, net_active_window);
    loop {
        let event = conn.wait_for_event()?;
        let Event::PropertyNotify(ev) = event else {
            continue;
        };
        if ev.atom != net_active_window {
            continue;
        }
        let win = active_window(&conn, root, net_active_window);
        if win == last {
            continue;
        }
        last = win;
        if tx
            .send(KeyEvent {
                kind: KeyKind::Reset,
                down: true,
                injected: false,
            })
            .is_err()
        {
            return Ok(()); // engine gone; shut down quietly
        }
    }
}

/// Current `_NET_ACTIVE_WINDOW` value, as in `LinuxFocus::active_window`.
fn active_window(conn: &RustConnection, root: u32, atom: u32) -> Option<u32> {
    let reply = conn
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    let vals: Vec<u32> = reply.value32()?.collect();
    vals.first().copied().filter(|w| *w != 0)
}
