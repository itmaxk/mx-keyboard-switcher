//! Dictionary lookup over the embedded, sorted word arrays.
use std::cmp::Reverse;

use crate::layout::Lang;
use crate::tables::{EN_RANKS, EN_WORDS, RU_RANKS, RU_WORDS};
use crate::usage::WordUsage;

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

/// Best ranked word that strictly extends lowercase `prefix`, or `None`.
///
/// Learned acceptance count takes precedence over static frequency rank.
/// When `minimum_count` is non-zero, only learned words meeting that threshold
/// are considered.
pub fn complete(
    prefix: &str,
    lang: Lang,
    usage: &WordUsage,
    minimum_count: u32,
) -> Option<&'static str> {
    if prefix.is_empty() {
        return None;
    }

    let ws = words(lang);
    let rs = ranks(lang);
    let mut best: Option<(Reverse<u32>, u16, &'static str)> = None;
    let mut consider = |index: usize, count: u32| {
        let word = ws[index];
        let candidate = (Reverse(count), rs[index], word);
        if best.is_none_or(|current| candidate < current) {
            best = Some(candidate);
        }
    };

    if minimum_count == 0 {
        let start = ws.partition_point(|word| *word < prefix);
        for (offset, word) in ws[start..].iter().enumerate() {
            if !word.starts_with(prefix) {
                break;
            }
            if word.len() > prefix.len() {
                consider(start + offset, usage.count(word, lang));
            }
        }
    } else {
        for (word, count) in usage.iter_lang(lang) {
            if count < minimum_count || !word.starts_with(prefix) || word.len() <= prefix.len() {
                continue;
            }
            if let Ok(index) = ws.binary_search(&word) {
                consider(index, count);
            }
        }
    }

    best.map(|(_, _, word)| word)
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
        let usage = WordUsage::default();
        // Many words start with "th"; the most frequent one must win,
        // not the alphabetically first.
        assert_eq!(complete("th", Lang::En, &usage, 0), Some("the"));
        assert_eq!(complete("wor", Lang::En, &usage, 0), Some("work"));
    }

    #[test]
    fn complete_russian() {
        let usage = WordUsage::default();
        let completion = complete("прив", Lang::Ru, &usage, 0).unwrap();
        assert!(completion.starts_with("прив"));
        assert!(completion.len() > "прив".len());
    }

    #[test]
    fn learned_ranking_uses_count_then_static_rank() {
        let mut usage = WordUsage::default();
        usage.increment("готово", Lang::Ru);
        usage.increment("готово", Lang::Ru);
        usage.increment("готовить", Lang::Ru);

        assert_eq!(complete("г", Lang::Ru, &usage, 1), Some("готово"));

        usage.increment("готовить", Lang::Ru);
        assert_eq!(complete("г", Lang::Ru, &usage, 1), Some("готово"));

        usage.increment("готовить", Lang::Ru);
        assert_eq!(complete("г", Lang::Ru, &usage, 1), Some("готовить"));
    }

    #[test]
    fn learned_candidates_must_match_the_full_prefix() {
        let mut usage = WordUsage::default();
        usage.increment("готово", Lang::Ru);
        usage.increment("готовить", Lang::Ru);

        let completion = complete("готи", Lang::Ru, &usage, 1);
        assert_ne!(completion, Some("готово"));
        assert!(completion.is_none_or(|word| word.starts_with("готи")));
    }

    #[test]
    fn learned_only_cutoff_excludes_untrained_words() {
        let usage = WordUsage::default();
        assert_eq!(complete("th", Lang::En, &usage, 1), None);

        let mut usage = usage;
        usage.increment("the", Lang::En);
        assert_eq!(complete("th", Lang::En, &usage, 2), None);
        assert_eq!(complete("th", Lang::En, &usage, 1), Some("the"));
    }

    #[test]
    fn complete_requires_strict_extension() {
        let mut usage = WordUsage::default();
        usage.increment("the", Lang::En);
        assert_ne!(complete("the", Lang::En, &usage, 0), Some("the"));
        assert_ne!(complete("the", Lang::En, &usage, 1), Some("the"));
        assert_eq!(complete("zzxqwk", Lang::En, &usage, 0), None);
    }

    #[test]
    fn complete_empty_prefix_is_none() {
        let usage = WordUsage::default();
        assert_eq!(complete("", Lang::En, &usage, 0), None);
        assert_eq!(complete("", Lang::En, &usage, 1), None);
    }
}
