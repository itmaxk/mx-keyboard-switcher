//! Input injection via CoreGraphics events. Text is typed as a Unicode string
//! (layout-independent); Backspace uses the Delete keycode. Every event carries
//! `MAGIC` in its user-data field so the tap can ignore it.
//!
//! A fresh `CGEventSource` is created per call rather than stored, because
//! `CGEventSource` is not `Send` and the injector lives on the engine thread.

use anyhow::{anyhow, Result};
use core_graphics::event::{CGEvent, CGEventTapLocation, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use super::MAGIC;

/// macOS virtual keycode for Delete/Backspace.
const KEYCODE_DELETE: u16 = 51;

/// macOS virtual keycode for Tab.
const KEYCODE_TAB: u16 = 48;

pub struct MacInjector;

impl MacInjector {
    pub fn new() -> Result<Self> {
        // Validate that we can create a source up front (surfaces permission
        // problems early), then drop it.
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("failed to create CGEventSource"))?;
        Ok(MacInjector)
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("failed to create CGEventSource"))
    }

    fn tag(event: &CGEvent) {
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
    }
}

impl crate::KeyInjector for MacInjector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        let source = Self::source()?;
        for _ in 0..n {
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(source.clone(), KEYCODE_DELETE, down)
                    .map_err(|_| anyhow!("failed to create keyboard event"))?;
                Self::tag(&event);
                event.post(CGEventTapLocation::HID);
            }
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        let source = Self::source()?;
        for ch in text.chars() {
            let utf16: Vec<u16> = ch.to_string().encode_utf16().collect();
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(source.clone(), 0, down)
                    .map_err(|_| anyhow!("failed to create keyboard event"))?;
                event.set_string_from_utf16_unchecked(&utf16);
                Self::tag(&event);
                event.post(CGEventTapLocation::HID);
            }
        }
        Ok(())
    }

    fn tab(&mut self) -> Result<()> {
        let source = Self::source()?;
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(source.clone(), KEYCODE_TAB, down)
                .map_err(|_| anyhow!("failed to create keyboard event"))?;
            Self::tag(&event);
            event.post(CGEventTapLocation::HID);
        }
        Ok(())
    }
}
