//! Linux X11 backend (v1). Uses XRecord for capture, XTEST for injection, and
//! XKB for layout read/switch. Wayland is not supported for global capture; on
//! a Wayland session we require XWayland (a set `DISPLAY`) and warn.

mod inject;
mod keymap;
mod record;
mod suppress;
mod xkb;

use anyhow::{bail, Result};
use mxks_core::hotkey::HotkeySpec;
use mxks_core::layout::Lang;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use crate::{Backend, FocusInfo, LayoutSwitcher};
use suppress::Suppress;

pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    check_display()?;

    let suppress = Suppress::new();

    let injector = inject::X11Injector::new(suppress.clone())?;
    let layout = XkbSwitcher(xkb::XkbLayout::new()?);
    let focus = LinuxFocus::new()?;
    let capture = record::X11Capture::new(suppress, hotkey);

    Ok(Backend {
        capture: Box::new(capture),
        injector: Box::new(injector),
        layout: Box::new(layout),
        focus: Box::new(focus),
    })
}

/// Refuse to start on a headless/Wayland session with no X server to talk to.
fn check_display() -> Result<()> {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let has_display = std::env::var("DISPLAY")
        .map(|d| !d.is_empty())
        .unwrap_or(false);
    if !has_display {
        bail!("no X11 DISPLAY found; MX Keyboard Switcher v1 requires X11 (or XWayland)");
    }
    if session == "wayland" {
        tracing::warn!(
            "Wayland session detected: running through XWayland. Global capture only \
             sees X11/XWayland windows; native Wayland apps are not covered."
        );
    }
    Ok(())
}

/// Adapter so the concrete XKB type satisfies the `LayoutSwitcher` trait.
struct XkbSwitcher(xkb::XkbLayout);

impl LayoutSwitcher for XkbSwitcher {
    fn current(&self) -> Result<Option<Lang>> {
        self.0.current()
    }
    fn switch_to(&mut self, lang: Lang) -> Result<()> {
        self.0.switch_to(lang)
    }
}

/// Best-effort focused-application name via `_NET_ACTIVE_WINDOW` + `WM_CLASS`.
struct LinuxFocus {
    conn: RustConnection,
    net_active_window: u32,
    root: u32,
}

impl LinuxFocus {
    fn new() -> Result<Self> {
        let (conn, screen_num) = RustConnection::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atom = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        Ok(LinuxFocus {
            conn,
            net_active_window: atom,
            root,
        })
    }

    fn active_window(&self) -> Option<u32> {
        let reply = self
            .conn
            .get_property(
                false,
                self.root,
                self.net_active_window,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let vals: Vec<u32> = reply.value32()?.collect();
        vals.first().copied().filter(|w| *w != 0)
    }
}

impl FocusInfo for LinuxFocus {
    fn is_password_field(&self) -> bool {
        // Not reliably detectable on X11; rely on config exclusions and gates.
        false
    }

    fn focused_app(&self) -> Option<String> {
        let win = self.active_window()?;
        let reply = self
            .conn
            .get_property(false, win, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64)
            .ok()?
            .reply()
            .ok()?;
        if reply.value.is_empty() {
            return None;
        }
        // WM_CLASS is "instance\0class\0"; keep it all, lowercased.
        let s = String::from_utf8_lossy(&reply.value).replace('\0', " ");
        Some(s.trim().to_lowercase())
    }
}
