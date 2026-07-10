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

/// Map a canonical hotkey name to a macOS virtual keycode. Note: Mac keyboards
/// have no Pause/Break key, so the default "PAUSE" returns `None`; macOS users
/// should configure a function key or a chord instead.
pub fn keycode_for_name(name: &str) -> Option<u16> {
    let kc = match name {
        "F1" => 0x7A,
        "F2" => 0x78,
        "F3" => 0x63,
        "F4" => 0x76,
        "F5" => 0x60,
        "F6" => 0x61,
        "F7" => 0x62,
        "F8" => 0x64,
        "F9" => 0x65,
        "F10" => 0x6D,
        "F11" => 0x67,
        "F12" => 0x6F,
        "F13" => 0x69,
        "F14" => 0x6B,
        "F15" => 0x71,
        "SPACE" => 0x31,
        "HOME" => 0x73,
        "END" => 0x77,
        "PAGEUP" => 0x74,
        "PAGEDOWN" => 0x79,
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_lowercase();
            let key = mxks_core::layout::char_to_key(c, mxks_core::layout::Lang::En)?;
            return phys_keycode(key);
        }
        _ => return None,
    };
    Some(kc)
}

/// Reverse lookup: macOS keycode for a tracked physical key.
fn phys_keycode(key: PhysKey) -> Option<u16> {
    LETTERS.iter().find(|(_, k)| *k == key).map(|(c, _)| *c)
}
