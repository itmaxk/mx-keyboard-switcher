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

    fn keyboard_event(source: &CGEventSource, keycode: u16, down: bool) -> Result<CGEvent> {
        let event = CGEvent::new_keyboard_event(source.clone(), keycode, down)
            .map_err(|_| anyhow!("failed to create keyboard event"))?;
        Self::tag(&event);
        Ok(event)
    }

    fn unicode_event(source: &CGEventSource, utf16: &[u16], down: bool) -> Result<CGEvent> {
        let event = Self::keyboard_event(source, 0, down)?;
        event.set_string_from_utf16_unchecked(utf16);
        Ok(event)
    }

    fn post_key(source: &CGEventSource, keycode: u16) -> Result<()> {
        for down in [true, false] {
            Self::keyboard_event(source, keycode, down)?.post(CGEventTapLocation::HID);
        }
        Ok(())
    }

    fn post_unicode(source: &CGEventSource, ch: char) -> Result<()> {
        let mut units = [0; 2];
        let utf16 = ch.encode_utf16(&mut units);
        for down in [true, false] {
            Self::unicode_event(source, utf16, down)?.post(CGEventTapLocation::HID);
        }
        Ok(())
    }
}

impl crate::KeyInjector for MacInjector {
    fn backspaces(&mut self, n: usize) -> Result<()> {
        let source = Self::source()?;
        for _ in 0..n {
            Self::post_key(&source, KEYCODE_DELETE)?;
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        let source = Self::source()?;
        for ch in text.chars() {
            Self::post_unicode(&source, ch)?;
        }
        Ok(())
    }

    fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
        let source = Self::source()?;
        for _ in 0..erase {
            Self::post_key(&source, KEYCODE_DELETE)?;
        }
        for ch in text.chars().chain(trailing.chars()) {
            Self::post_unicode(&source, ch)?;
        }
        Ok(())
    }

    fn tab(&mut self) -> Result<()> {
        let source = Self::source()?;
        Self::post_key(&source, KEYCODE_TAB)
    }
}
