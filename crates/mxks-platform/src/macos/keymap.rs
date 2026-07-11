//! Mapping between macOS virtual keycodes and [`PhysKey`], plus hotkey name →
//! keycode. macOS virtual keycodes are hardware-based (layout-independent).

use mxks_core::keycode::PhysKey;

pub const KC_DELETE: u16 = 0x33;
pub const KC_SPACE: u16 = 0x31;

const LETTERS: &[(u16, PhysKey)] = &[
    (0x0C, PhysKey::Q),
    (0x0D, PhysKey::W),
    (0x0E, PhysKey::E),
    (0x0F, PhysKey::R),
    (0x11, PhysKey::T),
    (0x10, PhysKey::Y),
    (0x20, PhysKey::U),
    (0x22, PhysKey::I),
    (0x1F, PhysKey::O),
    (0x23, PhysKey::P),
    (0x21, PhysKey::BracketL),
    (0x1E, PhysKey::BracketR),
    (0x00, PhysKey::A),
    (0x01, PhysKey::S),
    (0x02, PhysKey::D),
    (0x03, PhysKey::F),
    (0x05, PhysKey::G),
    (0x04, PhysKey::H),
    (0x26, PhysKey::J),
    (0x28, PhysKey::K),
    (0x25, PhysKey::L),
    (0x29, PhysKey::Semicolon),
    (0x27, PhysKey::Quote),
    (0x32, PhysKey::Backtick),
    (0x06, PhysKey::Z),
    (0x07, PhysKey::X),
    (0x08, PhysKey::C),
    (0x09, PhysKey::V),
    (0x0B, PhysKey::B),
    (0x2D, PhysKey::N),
    (0x2E, PhysKey::M),
    (0x2B, PhysKey::Comma),
    (0x2F, PhysKey::Period),
    (0x2C, PhysKey::Slash),
];

/// Keycodes that end a word (space, return, tab, digit row).
const BOUNDARIES: &[u16] = &[
    0x31, // space
    0x24, // return
    0x30, // tab
    0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19, 0x1D, // 1..0
    0x1B, 0x18, // minus, equals
];

pub fn phys_of(keycode: u16) -> Option<PhysKey> {
    LETTERS.iter().find(|(c, _)| *c == keycode).map(|(_, k)| *k)
}

pub fn is_boundary(keycode: u16) -> bool {
    BOUNDARIES.contains(&keycode)
}

/// Canonical hotkey name for a letter key, from its keycode (layout-independent).
pub fn key_letter_name(keycode: u16) -> Option<String> {
    let key = phys_of(keycode)?;
    let c = mxks_core::layout::key_to_char(key, mxks_core::layout::Lang::En)?;
    if c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase().to_string())
    } else {
        None
    }
}

/// Canonical hotkey name for a named (non-letter) macOS keycode. Mac keyboards
/// have no Pause/Break key, so function keys or chords are the practical choice.
pub fn named_keycode(keycode: u16) -> Option<&'static str> {
    Some(match keycode {
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        0x69 => "F13",
        0x6B => "F14",
        0x71 => "F15",
        0x31 => "SPACE",
        0x73 => "HOME",
        0x77 => "END",
        0x74 => "PAGEUP",
        0x79 => "PAGEDOWN",
        _ => return None,
    })
}
