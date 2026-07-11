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

/// Canonical hotkey name for a letter key, derived layout-independently from the
/// physical key (its English glyph, uppercased). `None` for non-letter keys.
pub fn key_letter_name(x_keycode: u8) -> Option<String> {
    let key = phys_of(x_keycode)?;
    let c = mxks_core::layout::key_to_char(key, mxks_core::layout::Lang::En)?;
    if c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase().to_string())
    } else {
        None
    }
}

/// True if `sym` is a modifier key (Shift/Control/Alt/Super/Caps/…). Pressing
/// these alone should neither reset the word buffer nor be recorded as a hotkey.
pub fn is_modifier_keysym(sym: u32) -> bool {
    (0xffe1..=0xffee).contains(&sym) // Shift_L..Hyper_R
        || sym == 0xfe03 // ISO_Level3_Shift (AltGr)
        || sym == 0xff7f // Num_Lock
}

/// Canonical hotkey name for a named (non-letter) X keysym, if recognized.
pub fn named_keysym(sym: u32) -> Option<&'static str> {
    Some(match sym {
        0xff13 | 0xff6b => "PAUSE",                          // Pause / Break
        0xff14 => "SCROLLLOCK",                              // Scroll_Lock
        0xff63 => "INSERT",                                  // Insert
        0xff50 => "HOME",                                    // Home
        0xff57 => "END",                                     // End
        0xff55 => "PAGEUP",                                  // Prior
        0xff56 => "PAGEDOWN",                                // Next
        0xff67 => "MENU",                                    // Menu
        0xffe5 => "CAPSLOCK",                                // Caps_Lock
        0x0020 => "SPACE",                                   // space
        0xffbe..=0xffc9 => return f_name(sym - 0xffbe + 1),  // F1..F12
        0xffca..=0xffe0 => return f_name(sym - 0xffca + 13), // F13..
        _ => return None,
    })
}

fn f_name(n: u32) -> Option<&'static str> {
    Some(match n {
        1 => "F1",
        2 => "F2",
        3 => "F3",
        4 => "F4",
        5 => "F5",
        6 => "F6",
        7 => "F7",
        8 => "F8",
        9 => "F9",
        10 => "F10",
        11 => "F11",
        12 => "F12",
        13 => "F13",
        14 => "F14",
        15 => "F15",
        16 => "F16",
        _ => return None,
    })
}
