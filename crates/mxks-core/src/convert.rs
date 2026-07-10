//! Layout-independent text conversion.
//!
//! Two entry points:
//! * [`render_keys`] — render a captured physical-key sequence through a target
//!   layout. This is the primary path: the word buffer already knows the keys.
//! * [`convert_str`] / [`convert_str_to`] — convert an existing string by first
//!   recovering the physical keys from its glyphs. Used by the manual hotkey
//!   (when only text is available) and by tests.

use crate::keycode::PhysKey;
use crate::layout::{char_to_key, is_letter_of, key_to_char, to_lower, Lang};

/// A captured keystroke: the physical key and whether Shift was held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stroke {
    pub key: PhysKey,
    pub shift: bool,
}

/// Render a physical-key sequence as text in `lang`, applying Shift as
/// uppercase. Keys with no glyph in `lang` are skipped.
pub fn render_keys(keys: &[Stroke], lang: Lang) -> String {
    let mut out = String::with_capacity(keys.len() * 2);
    for s in keys {
        if let Some(c) = key_to_char(s.key, lang) {
            if s.shift {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// Convert `s`, auto-detecting each character's source layout and rendering it
/// through the opposite one. Case is preserved. Characters that belong to
/// neither layout table are passed through unchanged.
pub fn convert_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        out.push_str(&convert_char(c));
    }
    out
}

/// Convert `s` assuming its letters were typed in `from` layout, rendering
/// through `from.other()`. Non-letters of `from` pass through unchanged.
pub fn convert_str_to(s: &str, from: Lang) -> String {
    let to = from.other();
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match char_to_key(c, from).and_then(|k| key_to_char(k, to)) {
            Some(t) => out.push_str(&apply_case(c, t)),
            None => out.push(c),
        }
    }
    out
}

fn convert_char(c: char) -> String {
    // Prefer whichever layout treats `c` as a letter; fall back to punctuation.
    for from in [Lang::En, Lang::Ru] {
        if is_letter_of(c, from) {
            if let Some(t) = char_to_key(c, from).and_then(|k| key_to_char(k, from.other())) {
                return apply_case(c, t);
            }
        }
    }
    for from in [Lang::En, Lang::Ru] {
        if let Some(t) = char_to_key(c, from).and_then(|k| key_to_char(k, from.other())) {
            return apply_case(c, t);
        }
    }
    c.to_string()
}

/// Give `target` the case of `source`.
fn apply_case(source: char, target: char) -> String {
    if source != to_lower(source) {
        target.to_uppercase().collect()
    } else {
        target.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::PhysKey::*;

    #[test]
    fn en_typed_as_ru() {
        assert_eq!(convert_str("ghbdtn"), "привет");
        assert_eq!(convert_str("Ghbdtn"), "Привет");
        assert_eq!(convert_str("vfvf"), "мама");
    }

    #[test]
    fn ru_typed_as_en() {
        assert_eq!(convert_str("руддщ"), "hello");
        assert_eq!(convert_str("Руддщ"), "Hello");
    }

    #[test]
    fn directed_conversion() {
        assert_eq!(convert_str_to("ghbdtn", Lang::En), "привет");
        assert_eq!(convert_str_to("руддщ", Lang::Ru), "hello");
    }

    #[test]
    fn round_trip() {
        let word = "программирование";
        let latin = convert_str_to(word, Lang::Ru);
        assert_eq!(convert_str_to(&latin, Lang::En), word);
    }

    #[test]
    fn render_from_keys() {
        // Physical keys G, H, T -> "пре" in Russian, "ght" in English.
        let keys = [
            Stroke {
                key: G,
                shift: false,
            },
            Stroke {
                key: H,
                shift: false,
            },
            Stroke {
                key: T,
                shift: false,
            },
        ];
        assert_eq!(render_keys(&keys, Lang::Ru), "пре");
        assert_eq!(render_keys(&keys, Lang::En), "ght");
    }

    #[test]
    fn shift_is_uppercase() {
        let keys = [
            Stroke {
                key: G,
                shift: true,
            },
            Stroke {
                key: H,
                shift: false,
            },
        ];
        assert_eq!(render_keys(&keys, Lang::Ru), "Пр");
    }

    #[test]
    fn passthrough_unknown() {
        assert_eq!(convert_str("123"), "123");
    }
}
