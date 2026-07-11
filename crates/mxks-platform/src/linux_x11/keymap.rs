//! Mapping between Linux evdev key codes, X11 keycodes, and [`PhysKey`], plus
//! classification of keys into engine event kinds.
//!
//! X11 keycode = evdev code + 8.

use mxks_core::keycode::PhysKey;

/// evdev offset applied to get X11 keycodes.
pub const X_OFFSET: u8 = 8;

/// X11 keycode of Backspace (evdev KEY_BACKSPACE = 14).
pub const KC_BACKSPACE: u8 = 14 + X_OFFSET;

/// X11 keycode of Space (evdev KEY_SPACE = 57).
pub const KC_SPACE: u8 = 57 + X_OFFSET;

/// X11 keycode of Left Shift (evdev KEY_LEFTSHIFT = 42).
pub const KC_SHIFT: u8 = 42 + X_OFFSET;

/// (evdev code, PhysKey) for every letter-block key we track.
const LETTERS: &[(u8, PhysKey)] = &[
    (16, PhysKey::Q),
    (17, PhysKey::W),
    (18, PhysKey::E),
    (19, PhysKey::R),
    (20, PhysKey::T),
    (21, PhysKey::Y),
    (22, PhysKey::U),
    (23, PhysKey::I),
    (24, PhysKey::O),
    (25, PhysKey::P),
    (26, PhysKey::BracketL),
    (27, PhysKey::BracketR),
    (30, PhysKey::A),
    (31, PhysKey::S),
    (32, PhysKey::D),
    (33, PhysKey::F),
    (34, PhysKey::G),
    (35, PhysKey::H),
    (36, PhysKey::J),
    (37, PhysKey::K),
    (38, PhysKey::L),
    (39, PhysKey::Semicolon),
    (40, PhysKey::Quote),
    (44, PhysKey::Z),
    (45, PhysKey::X),
    (46, PhysKey::C),
    (47, PhysKey::V),
    (48, PhysKey::B),
    (49, PhysKey::N),
    (50, PhysKey::M),
    (51, PhysKey::Comma),
    (52, PhysKey::Period),
    (53, PhysKey::Slash),
    (41, PhysKey::Backtick),
];

/// evdev codes of keys that end a word (space, enter, tab, and the digit row).
const BOUNDARIES: &[u8] = &[
    57, // space
    28, // enter
    15, // tab
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, // digits 1..0
    12, 13, // minus, equals
];

/// The X11 keycode that produces `PhysKey::A` — used to probe which XKB group is
/// English vs Russian.
pub const KC_PROBE: u8 = 30 + X_OFFSET;

/// Map an X11 keycode to a tracked letter [`PhysKey`], if any.
pub fn phys_of(x_keycode: u8) -> Option<PhysKey> {
    let evdev = x_keycode.checked_sub(X_OFFSET)?;
    LETTERS.iter().find(|(c, _)| *c == evdev).map(|(_, k)| *k)
}

/// X11 keycode for a tracked [`PhysKey`].
pub fn keycode_of(key: PhysKey) -> u8 {
    let evdev = LETTERS
        .iter()
        .find(|(_, k)| *k == key)
        .map(|(c, _)| *c)
        .unwrap_or(0);
    evdev + X_OFFSET
}

/// True if this X11 keycode is a word-boundary key.
pub fn is_boundary(x_keycode: u8) -> bool {
    match x_keycode.checked_sub(X_OFFSET) {
        Some(evdev) => BOUNDARIES.contains(&evdev),
        None => false,
    }
}

/// Map a canonical hotkey key name (see `mxks_core::hotkey`) to an X11 keycode.
pub fn keycode_for_name(name: &str) -> Option<u8> {
    let evdev = match name {
        "PAUSE" => 119,
        "SCROLLLOCK" => 70,
        "INSERT" => 110,
        "HOME" => 102,
        "END" => 107,
        "PAGEUP" => 104,
        "PAGEDOWN" => 109,
        "MENU" => 127,
        "CAPSLOCK" => 58,
        "SPACE" => 57,
        "F1" => 59,
        "F2" => 60,
        "F3" => 61,
        "F4" => 62,
        "F5" => 63,
        "F6" => 64,
        "F7" => 65,
        "F8" => 66,
        "F9" => 67,
        "F10" => 68,
        "F11" => 87,
        "F12" => 88,
        // Single letters A-Z map through the layout table.
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_lowercase();
            let key = mxks_core::layout::char_to_key(c, mxks_core::layout::Lang::En)?;
            return Some(keycode_of(key));
        }
        _ => return None,
    };
    Some(evdev + X_OFFSET)
}
