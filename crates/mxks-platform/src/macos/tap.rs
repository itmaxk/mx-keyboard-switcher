//! Global capture via `CGEventTap` (listen-only). Runs its own `CFRunLoop` on
//! the capture thread. Requires the Accessibility permission; without it the
//! tap creation fails and we return an actionable error.

use anyhow::{anyhow, Result};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, EventField,
};
use crossbeam_channel::Sender;
use mxks_core::hotkey::HotkeySpec;

use super::keymap;
use super::MAGIC;
use crate::event::{KeyEvent, KeyKind};
use crate::HotkeyControl;

pub struct MacCapture {
    hotkey: HotkeyControl,
}

impl MacCapture {
    pub fn new(hotkey: HotkeyControl) -> Self {
        MacCapture { hotkey }
    }
}

struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
}

fn mods(flags: CGEventFlags) -> Mods {
    Mods {
        shift: flags.contains(CGEventFlags::CGEventFlagShift),
        ctrl: flags.contains(CGEventFlags::CGEventFlagControl),
        alt: flags.contains(CGEventFlags::CGEventFlagAlternate),
        meta: flags.contains(CGEventFlags::CGEventFlagCommand),
    }
}

/// Canonical hotkey name for a keycode: letters via the physical key, otherwise
/// a recognized named keycode.
fn name_of(keycode: u16) -> Option<String> {
    if let Some(name) = keymap::key_letter_name(keycode) {
        return Some(name);
    }
    keymap::named_keycode(keycode).map(|s| s.to_string())
}

fn capture_spec(keycode: u16, m: &Mods) -> Option<HotkeySpec> {
    let name = name_of(keycode)?;
    let has_mod = m.ctrl || m.alt || m.meta;
    if name.len() == 1 && !has_mod {
        return None; // avoid a bare-letter hotkey
    }
    Some(HotkeySpec {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
        key: name,
    })
}

fn matches_hotkey(keycode: u16, m: &Mods, spec: &HotkeySpec) -> bool {
    match name_of(keycode) {
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

fn classify(keycode: u16, m: &Mods, spec: &HotkeySpec) -> Option<KeyKind> {
    if matches_hotkey(keycode, m, spec) {
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
    if keycode == keymap::KC_DELETE {
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

impl crate::KeyCapture for MacCapture {
    fn run(&mut self, tx: Sender<KeyEvent>) -> Result<()> {
        let hotkey = self.hotkey.clone();

        let tap = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![CGEventType::KeyDown],
            move |_proxy, etype, event| {
                if matches!(etype, CGEventType::KeyDown)
                    && event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) != MAGIC
                {
                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    let m = mods(event.get_flags());
                    if hotkey.is_capturing() {
                        if let Some(spec) = capture_spec(keycode, &m) {
                            hotkey.record(spec);
                        }
                    } else if let Some(kind) = classify(keycode, &m, &hotkey.current()) {
                        let _ = tx.send(KeyEvent {
                            kind,
                            down: true,
                            injected: false,
                        });
                    }
                }
                None
            },
        )
        .map_err(|_| {
            anyhow!("failed to create event tap; grant Accessibility permission to this app")
        })?;

        unsafe {
            let source = tap
                .mach_port
                .create_runloop_source(0)
                .map_err(|_| anyhow!("failed to create run loop source"))?;
            CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
            tap.enable();
            CFRunLoop::run_current();
        }
        Ok(())
    }
}
