//! Wrong-layout detection.
//!
//! Given a completed [`Word`] (a physical-key sequence plus the layout that was
//! active), decide whether it was actually meant for the *other* layout and, if
//! so, produce the corrected text.
//!
//! The design biases hard toward [`Verdict::Keep`]: a false correction (mangling
//! a word the user typed on purpose) is far more damaging than a missed one.
//! Signals, in priority order:
//!   1. Dictionary membership (fast, decisive).
//!   2. A margin in average per-bigram log-probability between the two renderings.
//!   3. An "impossible bigram" heuristic that catches gibberish like `ghbdtn`.

use crate::buffer::Word;
use crate::convert::render_keys;
use crate::dict;
use crate::layout::Lang;
use crate::tables::{EN_ALPHABET, EN_BIGRAM, RU_ALPHABET, RU_BIGRAM};

/// The outcome of analysing a word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Leave the word as typed.
    Keep,
    /// Replace the typed word with this corrected text.
    Correct(String),
    /// Not confident enough — the caller treats this like `Keep`.
    Uncertain,
}

/// Inputs to detection, assembled from the user's `Config`.
pub struct Params<'a> {
    pub min_word_len: usize,
    /// Required margin in average per-bigram log-probability (nats).
    pub threshold: f32,
    pub extra_en: &'a [String],
    pub extra_ru: &'a [String],
    /// Typed forms that must never be corrected.
    pub never_words: &'a [String],
}

impl Default for Params<'_> {
    fn default() -> Self {
        Params {
            min_word_len: 3,
            threshold: 2.0,
            extra_en: &[],
            extra_ru: &[],
            never_words: &[],
        }
    }
}

/// Any bigram log-probability at or below this is treated as "essentially never
/// occurs in this language" for the impossible-combo heuristic.
const IMPOSSIBLE_CUTOFF: f32 = -8.0;

/// Analyse a completed word and decide whether to correct it.
pub fn analyze(word: &Word, params: &Params) -> Verdict {
    if word.keys.len() < params.min_word_len {
        return Verdict::Keep;
    }

    let typed_lang = word.lang;
    let conv_lang = typed_lang.other();
    let typed = render_keys(&word.keys, typed_lang);
    let conv = render_keys(&word.keys, conv_lang);

    let typed_l = typed.to_lowercase();
    let conv_l = conv.to_lowercase();

    // Never-correct list (exact typed form).
    if params
        .never_words
        .iter()
        .any(|w| w.to_lowercase() == typed_l)
    {
        return Verdict::Keep;
    }

    let typed_valid =
        dict::contains(&typed, typed_lang) || in_list(extra(params, typed_lang), &typed_l);
    let conv_valid = dict::contains(&conv, conv_lang) || in_list(extra(params, conv_lang), &conv_l);

    // Decisive dictionary signals.
    if typed_valid && !conv_valid {
        return Verdict::Keep;
    }
    if conv_valid && !typed_valid {
        return Verdict::Correct(conv);
    }

    // Bigram scoring. If the converted form isn't representable in the other
    // layout (e.g. Cyrillic-only letters), it scores -inf and we keep.
    let s_conv = score(&conv_l, conv_lang);
    if !s_conv.is_finite() {
        return Verdict::Keep;
    }
    let s_typed = score(&typed_l, typed_lang);
    let margin = s_conv - s_typed;

    if margin > params.threshold {
        return Verdict::Correct(conv);
    }

    // Impossible-combo fast accept: the typed form contains a bigram that never
    // occurs in its language, the converted form doesn't, and conversion helps.
    if s_conv > s_typed
        && has_impossible(&typed_l, typed_lang)
        && !has_impossible(&conv_l, conv_lang)
    {
        return Verdict::Correct(conv);
    }

    Verdict::Keep
}

fn extra<'a>(params: &'a Params, lang: Lang) -> &'a [String] {
    match lang {
        Lang::En => params.extra_en,
        Lang::Ru => params.extra_ru,
    }
}

fn in_list(list: &[String], word: &str) -> bool {
    list.iter().any(|w| w.to_lowercase() == word)
}

fn alphabet(lang: Lang) -> &'static str {
    match lang {
        Lang::En => EN_ALPHABET,
        Lang::Ru => RU_ALPHABET,
    }
}

fn char_index(c: char, lang: Lang) -> Option<usize> {
    alphabet(lang).chars().position(|a| a == c)
}

/// Look up a bigram log-probability from the right fixed-size table.
fn bigram(lang: Lang, i: usize, j: usize) -> f32 {
    match lang {
        Lang::En => EN_BIGRAM[i][j],
        Lang::Ru => RU_BIGRAM[i][j],
    }
}

/// Average per-transition log-probability of `s` under `lang`'s bigram model,
/// including boundary transitions. Returns `-inf` if `s` contains a character
/// that isn't a letter of `lang`.
fn score(s: &str, lang: Lang) -> f32 {
    let n = alphabet(lang).chars().count(); // boundary symbol index
    let mut prev = n;
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for c in s.chars() {
        let Some(cur) = char_index(c, lang) else {
            return f32::NEG_INFINITY;
        };
        sum += bigram(lang, prev, cur);
        prev = cur;
        count += 1;
    }
    if count == 0 {
        return f32::NEG_INFINITY;
    }
    sum += bigram(lang, prev, n); // transition to end-of-word
    count += 1;
    sum / count as f32
}

/// True if any bigram in `s` (including boundaries) is effectively impossible
/// in `lang`.
fn has_impossible(s: &str, lang: Lang) -> bool {
    let n = alphabet(lang).chars().count();
    let mut prev = n;
    for c in s.chars() {
        let Some(cur) = char_index(c, lang) else {
            return true;
        };
        if bigram(lang, prev, cur) <= IMPOSSIBLE_CUTOFF {
            return true;
        }
        prev = cur;
    }
    bigram(lang, prev, n) <= IMPOSSIBLE_CUTOFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{convert_str_to, Stroke};
    use crate::layout::char_to_key;

    /// Build a `Word` as if `text` were typed while `lang` was active.
    fn typed_word(text: &str, lang: Lang) -> Word {
        let keys = text
            .chars()
            .map(|c| {
                let lower = c.to_lowercase().next().unwrap();
                let key = char_to_key(lower, lang)
                    .unwrap_or_else(|| panic!("no key for {c:?} in {lang:?}"));
                Stroke {
                    key,
                    shift: c != lower,
                }
            })
            .collect();
        Word { keys, lang }
    }

    #[test]
    fn corrects_obvious_wrong_layout() {
        // "ghbdtn" typed while EN active is really "привет".
        let w = typed_word("ghbdtn", Lang::En);
        assert_eq!(
            analyze(&w, &Params::default()),
            Verdict::Correct("привет".to_string())
        );
    }

    #[test]
    fn keeps_valid_english() {
        for word in ["hello", "world", "the", "keyboard", "switch"] {
            let w = typed_word(word, Lang::En);
            assert_eq!(
                analyze(&w, &Params::default()),
                Verdict::Keep,
                "should keep {word}"
            );
        }
    }

    #[test]
    fn keeps_valid_russian() {
        for word in ["привет", "мир", "клавиатура", "спасибо"] {
            let w = typed_word(word, Lang::Ru);
            assert_eq!(
                analyze(&w, &Params::default()),
                Verdict::Keep,
                "should keep {word}"
            );
        }
    }

    #[test]
    fn corrects_ru_typed_as_en() {
        // "руддщ" typed while RU active is really "hello".
        let w = typed_word("руддщ", Lang::Ru);
        assert_eq!(
            analyze(&w, &Params::default()),
            Verdict::Correct("hello".to_string())
        );
    }

    #[test]
    fn respects_min_len() {
        let w = typed_word("go", Lang::En);
        assert_eq!(analyze(&w, &Params::default()), Verdict::Keep);
    }

    #[test]
    fn never_words_are_kept() {
        let never = vec!["ghb".to_string()];
        let params = Params {
            never_words: &never,
            min_word_len: 3,
            ..Params::default()
        };
        let w = typed_word("ghb", Lang::En);
        assert_eq!(analyze(&w, &params), Verdict::Keep);
    }

    #[test]
    fn sanity_convert_helpers() {
        assert_eq!(convert_str_to("ghbdtn", Lang::En), "привет");
    }
}
