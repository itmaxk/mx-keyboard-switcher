//! Per-word buffer of physical keystrokes.
//!
//! The engine feeds every key event here. The buffer accumulates letter
//! keystrokes into the "current word" and reports a completed word when a
//! boundary key arrives. It stays in sync with the visible text by tracking
//! Backspace.

use crate::convert::Stroke;
use crate::layout::Lang;

/// Maximum keystrokes tracked in a single word. Longer words are simply not
/// eligible for correction (tracking resumes at the next boundary or reset).
const MAX_WORD: usize = 64;

/// What a single fed event means for word tracking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// A letter keystroke on the main block.
    Letter(Stroke),
    /// Backspace: remove the last keystroke.
    Backspace,
    /// A key that ends the current word (space, enter, tab, punctuation, digit).
    Boundary,
    /// Anything that invalidates the buffer: arrows, mouse click, Esc, a
    /// modifier chord (Ctrl/Alt/Cmd+key), focus change, or a user layout switch.
    Reset,
}

/// A completed word ready for detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub keys: Vec<Stroke>,
    /// The active layout when the word started.
    pub lang: Lang,
}

/// Accumulates keystrokes into words.
#[derive(Debug)]
pub struct WordBuffer {
    keys: Vec<Stroke>,
    lang: Lang,
    /// Layout at the moment the current word's first key was pressed.
    word_start_lang: Lang,
    /// Once a word overflows, its untracked prefix makes every suffix unsafe
    /// to correct. Resume tracking only after a boundary or explicit reset.
    overflowed: bool,
}

impl WordBuffer {
    pub fn new(lang: Lang) -> Self {
        WordBuffer {
            keys: Vec::with_capacity(MAX_WORD),
            lang,
            word_start_lang: lang,
            overflowed: false,
        }
    }

    /// Current active layout (updated by the engine when the user switches).
    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
    }

    /// True if no keystrokes are buffered for the current word.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The word being typed right now (may be empty).
    pub fn current(&self) -> Option<Word> {
        if self.keys.is_empty() {
            None
        } else {
            Some(Word {
                keys: self.keys.clone(),
                lang: self.word_start_lang,
            })
        }
    }

    /// Feed one event. Returns the completed word when `event` is a boundary.
    pub fn feed(&mut self, event: Event) -> Option<Word> {
        match event {
            Event::Letter(stroke) => {
                if self.overflowed {
                    return None;
                }
                if self.keys.is_empty() {
                    self.word_start_lang = self.lang;
                }
                if self.keys.len() >= MAX_WORD {
                    self.keys.clear();
                    self.overflowed = true;
                } else {
                    self.keys.push(stroke);
                }
                None
            }
            Event::Backspace => {
                self.keys.pop();
                None
            }
            Event::Boundary => {
                self.overflowed = false;
                if self.keys.is_empty() {
                    None
                } else {
                    let word = Word {
                        keys: std::mem::take(&mut self.keys),
                        lang: self.word_start_lang,
                    };
                    Some(word)
                }
            }
            Event::Reset => {
                self.clear();
                None
            }
        }
    }

    /// Clear the current word (e.g. after a manual conversion consumes it).
    pub fn clear(&mut self) {
        self.keys.clear();
        self.overflowed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::PhysKey::*;

    fn letter(k: crate::keycode::PhysKey) -> Event {
        Event::Letter(Stroke {
            key: k,
            shift: false,
        })
    }

    #[test]
    fn accumulates_and_emits_on_boundary() {
        let mut b = WordBuffer::new(Lang::En);
        assert!(b.feed(letter(H)).is_none());
        assert!(b.feed(letter(I)).is_none());
        let word = b.feed(Event::Boundary).unwrap();
        assert_eq!(word.keys.len(), 2);
        assert_eq!(word.lang, Lang::En);
    }

    #[test]
    fn backspace_shrinks() {
        let mut b = WordBuffer::new(Lang::En);
        b.feed(letter(H));
        b.feed(letter(I));
        b.feed(Event::Backspace);
        let word = b.feed(Event::Boundary).unwrap();
        assert_eq!(word.keys.len(), 1);
    }

    #[test]
    fn reset_discards() {
        let mut b = WordBuffer::new(Lang::En);
        b.feed(letter(H));
        assert!(b.feed(Event::Reset).is_none());
        assert!(b.feed(Event::Boundary).is_none());
    }

    #[test]
    fn empty_boundary_is_noop() {
        let mut b = WordBuffer::new(Lang::En);
        assert!(b.feed(Event::Boundary).is_none());
    }

    #[test]
    fn word_start_lang_is_captured() {
        let mut b = WordBuffer::new(Lang::En);
        b.feed(letter(H));
        b.set_lang(Lang::Ru); // user switches mid-word; word keeps its start lang
        let word = b.feed(Event::Boundary).unwrap();
        assert_eq!(word.lang, Lang::En);
    }

    #[test]
    fn maximum_length_word_is_still_eligible() {
        let mut b = WordBuffer::new(Lang::En);
        for _ in 0..MAX_WORD {
            b.feed(letter(A));
        }
        assert_eq!(b.feed(Event::Boundary).unwrap().keys.len(), MAX_WORD);
    }

    #[test]
    fn overflow_does_not_expose_a_correctable_suffix() {
        let mut b = WordBuffer::new(Lang::En);
        for _ in 0..MAX_WORD + 1 {
            b.feed(letter(A));
        }
        // This suffix would otherwise be corrected from "ghbdtn" to "привет"
        // inside a longer token, despite the maximum-word-length guard.
        for key in [G, H, B, D, T, N] {
            b.feed(letter(key));
        }
        assert!(b.is_empty());
        assert!(b.current().is_none());
        assert!(b.feed(Event::Boundary).is_none());

        b.set_lang(Lang::Ru);
        b.feed(letter(H));
        let next = b.feed(Event::Boundary).unwrap();
        assert_eq!(next.keys.len(), 1);
        assert_eq!(next.lang, Lang::Ru);
    }

    #[test]
    fn backspace_cannot_reenable_an_untracked_word() {
        let mut b = WordBuffer::new(Lang::En);
        for _ in 0..MAX_WORD * 3 {
            b.feed(letter(A));
        }
        b.feed(Event::Backspace);
        b.feed(letter(H));
        assert!(b.current().is_none());
        assert!(b.feed(Event::Boundary).is_none());
    }

    #[test]
    fn explicit_reset_or_clear_resumes_tracking_after_overflow() {
        for reset in [true, false] {
            let mut b = WordBuffer::new(Lang::En);
            for _ in 0..MAX_WORD + 1 {
                b.feed(letter(A));
            }
            if reset {
                b.feed(Event::Reset);
            } else {
                b.clear();
            }
            b.feed(letter(H));
            assert_eq!(b.feed(Event::Boundary).unwrap().keys.len(), 1);
        }
    }
}
