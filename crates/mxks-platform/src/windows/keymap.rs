//! Mapping between Windows PS/2 set-1 scan codes and [`PhysKey`], plus hotkey
//! name → virtual-key mapping.
//!
//! We key on the hardware **scan code** (layout-independent), not the virtual
//! key (which depends on the active layout).

use mxks_core::keycode::PhysKey;

pub const SC_BACKSPACE: u32 = 0x0E;
pub const SC_SPACE: u32 = 0x39;

/// (scan code, PhysKey) for the tracked letter-block keys.
const LETTERS: &[(u32, PhysKey)] = &[
    (0x10, PhysKey::Q),
    (0x11, PhysKey::W),
    (0x12, PhysKey::E),
    (0x13, PhysKey::R),
    (0x14, PhysKey::T),
    (0x15, PhysKey::Y),
    (0x16, PhysKey::U),
    (0x17, PhysKey::I),
    (0x18, PhysKey::O),
    (0x19, PhysKey::P),
    (0x1A, PhysKey::BracketL),
    (0x1B, PhysKey::BracketR),
    (0x1E, PhysKey::A),
    (0x1F, PhysKey::S),
    (0x20, PhysKey::D),
    (0x21, PhysKey::F),
    (0x22, PhysKey::G),
    (0x23, PhysKey::H),
    (0x24, PhysKey::J),
    (0x25, PhysKey::K),
    (0x26, PhysKey::L),
    (0x27, PhysKey::Semicolon),
    (0x28, PhysKey::Quote),
    (0x29, PhysKey::Backtick),
    (0x2C, PhysKey::Z),
    (0x2D, PhysKey::X),
    (0x2E, PhysKey::C),
    (0x2F, PhysKey::V),
    (0x30, PhysKey::B),
    (0x31, PhysKey::N),
    (0x32, PhysKey::M),
    (0x33, PhysKey::Comma),
    (0x34, PhysKey::Period),
    (0x35, PhysKey::Slash),
];

/// Scan codes that end a word (space, enter, tab, digit row).
const BOUNDARIES: &[u32] = &[
    0x39, // space
    0x1C, // enter
    0x0F, // tab
    0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, // 1..0
    0x0C, 0x0D, // minus, equals
];

pub fn phys_of(scan: u32) -> Option<PhysKey> {
    LETTERS.iter().find(|(s, _)| *s == scan).map(|(_, k)| *k)
}

pub fn is_boundary(scan: u32) -> bool {
    BOUNDARIES.contains(&scan)
}

/// Map a canonical hotkey key name to a Win32 virtual-key code.
pub fn vk_for_name(name: &str) -> Option<u16> {
    let vk = match name {
        "PAUSE" => 0x13,      // VK_PAUSE
        "SCROLLLOCK" => 0x91, // VK_SCROLL
        "INSERT" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "MENU" => 0x5D, // VK_APPS
        "CAPSLOCK" => 0x14,
        "SPACE" => 0x20,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_uppercase();
            if c.is_ascii_alphabetic() {
                c as u16 // VK for 'A'..'Z' equals the ASCII code
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(vk)
}
