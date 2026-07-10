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

pub struct MacCapture {
    hotkey: HotkeySpec,
}

impl MacCapture {
    pub fn new(hotkey: HotkeySpec) -> Self {
        MacCapture { hotkey }
    }
}

fn classify(
    keycode: u16,
    flags: CGEventFlags,
    hotkey_kc: Option<u16>,
    hotkey: &HotkeySpec,
) -> Option<KeyKind> {
    let shift = flags.contains(CGEventFlags::CGEventFlagShift);
    let ctrl = flags.contains(CGEventFlags::CGEventFlagControl);
    let alt = flags.contains(CGEventFlags::CGEventFlagAlternate);
    let meta = flags.contains(CGEventFlags::CGEventFlagCommand);

    if Some(keycode) == hotkey_kc
        && ctrl == hotkey.ctrl
        && shift == hotkey.shift
        && alt == hotkey.alt
        && meta == hotkey.meta
    {
        return Some(KeyKind::Hotkey);
    }
    if ctrl || alt || meta {
        return Some(KeyKind::Reset);
    }
    if let Some(key) = keymap::phys_of(keycode) {
        return Some(KeyKind::Letter { key, shift });
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
        let hotkey_kc = keymap::keycode_for_name(&hotkey.key);

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
                    if let Some(kind) = classify(keycode, event.get_flags(), hotkey_kc, &hotkey) {
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
