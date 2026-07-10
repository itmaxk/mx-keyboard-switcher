//! Applies corrections: erase the typed word, switch the system layout, retype
//! the converted text. Owns the injector and layout switcher.

use anyhow::Result;
use mxks_core::buffer::Word;
use mxks_core::convert::render_keys;
use mxks_platform::{KeyInjector, LayoutSwitcher};

pub struct Corrector {
    injector: Box<dyn KeyInjector>,
    layout: Box<dyn LayoutSwitcher>,
}

impl Corrector {
    pub fn new(injector: Box<dyn KeyInjector>, layout: Box<dyn LayoutSwitcher>) -> Self {
        Corrector { injector, layout }
    }

    /// Replace a just-completed word with `converted`, switching to the other
    /// layout. `trailing` is the separator already on screen after the word
    /// (e.g. `" "`); it is erased and re-typed so the caret ends up past it.
    pub fn autocorrect(&mut self, word: &Word, converted: &str, trailing: &str) -> Result<()> {
        let erase = word.keys.len() + trailing.chars().count();
        self.injector.backspaces(erase)?;
        self.layout.switch_to(word.lang.other())?;
        self.injector.type_text(converted)?;
        if !trailing.is_empty() {
            self.injector.type_text(trailing)?;
        }
        Ok(())
    }

    /// Manually convert an in-progress word (no separator typed yet): erase it,
    /// switch layout, retype it through the other layout. Returns the converted
    /// text.
    pub fn manual(&mut self, word: &Word) -> Result<String> {
        let converted = render_keys(&word.keys, word.lang.other());
        self.injector.backspaces(word.keys.len())?;
        self.layout.switch_to(word.lang.other())?;
        self.injector.type_text(&converted)?;
        Ok(converted)
    }

    /// Read the current system layout, if it is EN or RU.
    pub fn current_layout(&self) -> Option<mxks_core::layout::Lang> {
        self.layout.current().ok().flatten()
    }
}
