//! Platform-neutral key event delivered from a capture backend to the engine.

use mxks_core::keycode::PhysKey;

/// A single key transition observed by a capture backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The logical action this key represents for word tracking.
    pub kind: KeyKind,
    /// True on key-down, false on key-up. The engine acts on key-down.
    pub down: bool,
    /// True if this event was synthesized by our own injector (should be
    /// ignored by the engine to avoid feedback loops). Backends set this via
    /// their tagging mechanism where available.
    pub injected: bool,
}

/// Classification of a physical key for the engine, decided by the backend from
/// the physical key and current modifier state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// A letter key on the main block, with Shift state.
    Letter { key: PhysKey, shift: bool },
    /// Backspace.
    Backspace,
    /// A word-boundary key. `sep` is the separator character to re-type after a
    /// correction (`Some(' ')` for Space); `None` means end the word but do not
    /// auto-correct at this boundary (Enter, Tab, punctuation, digits).
    Boundary { sep: Option<char> },
    /// The user's convert-last-word hotkey fired.
    Hotkey,
    /// Something that invalidates the word buffer (arrows, Esc, modifier chord,
    /// mouse click, focus/layout change).
    Reset,
}
