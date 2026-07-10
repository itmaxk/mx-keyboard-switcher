//! Static layout tables mapping [`PhysKey`] to characters for the
//! English (US-QWERTY) and Russian (ЙЦУКЕН) layouts.

use crate::keycode::PhysKey;

/// A language / layout selector.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    /// The opposite layout.
    pub fn other(self) -> Lang {
        match self {
            Lang::En => Lang::Ru,
            Lang::Ru => Lang::En,
        }
    }
}

/// One row of the layout table: a physical key and the lowercase glyph it
/// produces in each layout (`None` when the key produces no letter there).
struct Row {
    key: PhysKey,
    en: char,
    ru: Option<char>,
}

/// Lowercase glyphs per physical key. Shifted/uppercase forms are derived via
/// `char::to_uppercase`, so only lowercase is stored.
///
/// Russian mapping follows the standard ЙЦУКЕН positions. Keys that are
/// punctuation in Russian (e.g. `/`) have `ru: None` and act as word boundaries.
const TABLE: [Row; 34] = {
    use PhysKey::*;
    macro_rules! row {
        ($k:expr, $en:expr, $ru:expr) => {
            Row {
                key: $k,
                en: $en,
                ru: Some($ru),
            }
        };
        ($k:expr, $en:expr) => {
            Row {
                key: $k,
                en: $en,
                ru: None,
            }
        };
    }
    [
        row!(Q, 'q', 'й'),
        row!(W, 'w', 'ц'),
        row!(E, 'e', 'у'),
        row!(R, 'r', 'к'),
        row!(T, 't', 'е'),
        row!(Y, 'y', 'н'),
        row!(U, 'u', 'г'),
        row!(I, 'i', 'ш'),
        row!(O, 'o', 'щ'),
        row!(P, 'p', 'з'),
        row!(BracketL, '[', 'х'),
        row!(BracketR, ']', 'ъ'),
        row!(A, 'a', 'ф'),
        row!(S, 's', 'ы'),
        row!(D, 'd', 'в'),
        row!(F, 'f', 'а'),
        row!(G, 'g', 'п'),
        row!(H, 'h', 'р'),
        row!(J, 'j', 'о'),
        row!(K, 'k', 'л'),
        row!(L, 'l', 'д'),
        row!(Semicolon, ';', 'ж'),
        row!(Quote, '\'', 'э'),
        row!(Z, 'z', 'я'),
        row!(X, 'x', 'ч'),
        row!(C, 'c', 'с'),
        row!(V, 'v', 'м'),
        row!(B, 'b', 'и'),
        row!(N, 'n', 'т'),
        row!(M, 'm', 'ь'),
        row!(Comma, ',', 'б'),
        row!(Period, '.', 'ю'),
        row!(Slash, '/'),
        row!(Backtick, '`', 'ё'),
    ]
};

/// The lowercase glyph produced by `key` in `lang`, if any.
pub fn key_to_char(key: PhysKey, lang: Lang) -> Option<char> {
    let row = &TABLE[key as usize];
    debug_assert_eq!(row.key, key, "TABLE order must match PhysKey discriminants");
    match lang {
        Lang::En => Some(row.en),
        Lang::Ru => row.ru,
    }
}

/// The physical key that produces `c` (case-insensitively) in `lang`, if any.
pub fn char_to_key(c: char, lang: Lang) -> Option<PhysKey> {
    let lc = to_lower(c);
    for row in &TABLE {
        let glyph = match lang {
            Lang::En => Some(row.en),
            Lang::Ru => row.ru,
        };
        if glyph == Some(lc) {
            return Some(row.key);
        }
    }
    None
}

/// Lowercase a single char, handling both ASCII and Cyrillic.
pub fn to_lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// True if `c` (case-insensitively) is a letter in `lang`.
pub fn is_letter_of(c: char, lang: Lang) -> bool {
    char_to_key(c, lang)
        .and_then(|k| key_to_char(k, lang))
        .map(|g| g.is_alphabetic())
        .unwrap_or(false)
}
