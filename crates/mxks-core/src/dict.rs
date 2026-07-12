//! Dictionary lookup over the embedded, sorted word arrays.

use crate::layout::Lang;
use crate::tables::{EN_RANKS, EN_WORDS, RU_RANKS, RU_WORDS};

/// Sorted word slice for `lang`.
fn words(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::En => EN_WORDS,
        Lang::Ru => RU_WORDS,
    }
}

/// Frequency ranks parallel to `words(lang)` (rank 0 = most frequent).
fn ranks(lang: Lang) -> &'static [u16] {
    match lang {
        Lang::En => EN_RANKS,
        Lang::Ru => RU_RANKS,
    }
}

/// True if `word` (case-insensitively) is in the embedded dictionary for `lang`.
pub fn contains(word: &str, lang: Lang) -> bool {
    let w = word.to_lowercase();
    words(lang)
        .binary_search_by(|e| (*e).cmp(w.as_str()))
        .is_ok()
}

/// Most frequent word that strictly extends lowercase `prefix`, or `None`.
///
/// A word equal to `prefix` is not a completion; the caller decides how much
/// of the prefix is required before asking.
pub fn complete(prefix: &str, lang: Lang) -> Option<&'static str> {
    if prefix.is_empty() {
        return None;
    }
    let ws = words(lang);
    let rs = ranks(lang);
    let start = ws.partition_point(|w| *w < prefix);
    let mut best: Option<(u16, &'static str)> = None;
    for i in start..ws.len() {
        let w = ws[i];
        if !w.starts_with(prefix) {
            break;
        }
        if w.len() > prefix.len() && best.is_none_or(|(r, _)| rs[i] < r) {
            best = Some((rs[i], w));
        }
    }
    best.map(|(_, w)| w)
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

    #[test]
    fn complete_prefers_frequency_over_alphabet() {
        // Many words start with "th"; the most frequent one must win,
        // not the alphabetically first.
        assert_eq!(complete("th", Lang::En), Some("the"));
        assert_eq!(complete("wor", Lang::En), Some("work"));
    }

    #[test]
    fn complete_russian() {
        let c = complete("прив", Lang::Ru).unwrap();
        assert!(c.starts_with("прив"));
        assert!(c.chars().count() > "прив".chars().count());
    }

    #[test]
    fn complete_requires_strict_extension() {
        // A completion always extends the prefix; the prefix itself never counts.
        if let Some(c) = complete("the", Lang::En) {
            assert!(c.starts_with("the") && c.len() > 3);
        }
        assert_eq!(complete("zzxqwk", Lang::En), None);
    }

    #[test]
    fn complete_empty_prefix_is_none() {
        assert_eq!(complete("", Lang::En), None);
    }
}
