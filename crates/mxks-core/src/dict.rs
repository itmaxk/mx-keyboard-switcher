//! Dictionary lookup over the embedded, sorted word arrays.

use crate::layout::Lang;
use crate::tables::{EN_WORDS, RU_WORDS};

/// Sorted word slice for `lang`.
fn words(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::En => EN_WORDS,
        Lang::Ru => RU_WORDS,
    }
}

/// True if `word` (case-insensitively) is in the embedded dictionary for `lang`.
pub fn contains(word: &str, lang: Lang) -> bool {
    let w = word.to_lowercase();
    words(lang)
        .binary_search_by(|e| (*e).cmp(w.as_str()))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_words_present() {
        assert!(contains("hello", Lang::En));
        assert!(contains("Hello", Lang::En));
        assert!(contains("привет", Lang::Ru));
    }

    #[test]
    fn gibberish_absent() {
        assert!(!contains("ghbdtn", Lang::En));
        assert!(!contains("zzxqwk", Lang::En));
    }
}
