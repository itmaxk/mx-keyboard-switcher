//! Corpus regression test — the product-defining quality gate.
//!
//! * **Zero false positives**: none of a large set of genuinely-valid EN and RU
//!   words may be "corrected". A false correction is the worst failure mode.
//! * **High true-positive rate**: most words typed in the wrong layout must be
//!   detected and corrected.
//!
//! The word lists are drawn from the embedded dictionaries, so this test tracks
//! the same data the detector uses.

use mxks_core::buffer::Word;
use mxks_core::convert::Stroke;
use mxks_core::detect::{analyze, Params, Verdict};
use mxks_core::layout::{char_to_key, Lang};
use mxks_core::tables::{EN_WORDS, RU_WORDS};

/// Build a `Word` as if `text` were typed while `lang` was the active layout.
/// Returns `None` if any char has no key in `lang` (skip such words).
fn typed_word(text: &str, lang: Lang) -> Option<Word> {
    let mut keys = Vec::new();
    for c in text.chars() {
        let lower = c.to_lowercase().next().unwrap();
        let key = char_to_key(lower, lang)?;
        keys.push(Stroke {
            key,
            shift: c != lower,
        });
    }
    Some(Word { keys, lang })
}

#[test]
fn zero_false_positives_on_valid_words() {
    let params = Params::default();
    let mut fp = Vec::new();

    // Valid English words typed in EN must be kept.
    for w in EN_WORDS.iter().take(5000) {
        if let Some(word) = typed_word(w, Lang::En) {
            if let Verdict::Correct(to) = analyze(&word, &params) {
                fp.push(format!("EN '{w}' -> '{to}'"));
            }
        }
    }
    // Valid Russian words typed in RU must be kept.
    for w in RU_WORDS.iter().take(5000) {
        if let Some(word) = typed_word(w, Lang::Ru) {
            if let Verdict::Correct(to) = analyze(&word, &params) {
                fp.push(format!("RU '{w}' -> '{to}'"));
            }
        }
    }

    let total = 10_000;
    assert!(
        fp.is_empty(),
        "{} false positives out of {}:\n{}",
        fp.len(),
        total,
        fp.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn high_true_positive_rate() {
    let params = Params::default();

    // Take common words (length >= min) and "type them in the wrong layout":
    // a RU word typed while EN is active still produces the RU keystrokes, so
    // analyze() should recover the RU word. We simulate by building the word
    // from the target-language keystrokes but tagging the *wrong* active layout.
    let mut detected = 0usize;
    let mut total = 0usize;

    // RU words typed while EN active (the classic case).
    for w in RU_WORDS.iter().take(3000) {
        if w.chars().count() < 4 {
            continue;
        }
        // Keys that produce this RU word:
        let Some(ru_word) = typed_word(w, Lang::Ru) else {
            continue;
        };
        // Same keys, but the user had EN active by mistake:
        let mistyped = Word {
            keys: ru_word.keys.clone(),
            lang: Lang::En,
        };
        total += 1;
        if let Verdict::Correct(to) = analyze(&mistyped, &params) {
            if to == *w {
                detected += 1;
            }
        }
    }

    let rate = detected as f64 / total as f64;
    assert!(
        rate >= 0.90,
        "true-positive rate {:.1}% ({}/{}) below 90%",
        rate * 100.0,
        detected,
        total
    );
}
