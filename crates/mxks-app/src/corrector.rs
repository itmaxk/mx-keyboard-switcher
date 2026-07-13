//! Applies corrections: erase the currently displayed text, switch the system
//! layout, retype the text through the target layout. Owns the injector and
//! layout switcher.

use anyhow::Result;
use mxks_core::convert::{render_keys, Stroke};
use mxks_core::layout::Lang;
use mxks_platform::{KeyInjector, LayoutSwitcher};

pub struct Corrector {
    injector: Box<dyn KeyInjector>,
    layout: Box<dyn LayoutSwitcher>,
}

impl Corrector {
    pub fn new(injector: Box<dyn KeyInjector>, layout: Box<dyn LayoutSwitcher>) -> Self {
        Corrector { injector, layout }
    }

    /// Re-render `keys` from its current layout `from` into `to`: erase what is
    /// on screen (the `from` rendering plus `trailing`), switch the system
    /// layout to `to`, and type the `to` rendering plus `trailing`.
    ///
    /// Erase length is computed from the actual rendered text (not the key
    /// count), so it is correct even when the two layouts render different
    /// character counts.
    pub fn convert(&mut self, keys: &[Stroke], from: Lang, to: Lang, trailing: &str) -> Result<()> {
        let current = render_keys(keys, from);
        let erase = current.chars().count() + trailing.chars().count();
        self.injector.backspaces(erase)?;
        self.layout.switch_to(to)?;
        let text = render_keys(keys, to);
        self.injector.type_text(&text)?;
        if !trailing.is_empty() {
            self.injector.type_text(trailing)?;
        }
        Ok(())
    }

    /// Read the current system layout, if it is EN or RU.
    pub fn current_layout(&self) -> Option<Lang> {
        self.layout.current().ok().flatten()
    }

    /// Insert a completion `remainder` whose letters are all in `lang`.
    ///
    /// The X11 injector replays *physical key positions*, so it only produces
    /// `lang`'s letters when the active layout is already `lang`. `word.lang`
    /// usually matches the active layout, but they drift apart if the user
    /// switched layout mid-word (or a layout read failed), which would otherwise
    /// inject the wrong script (e.g. an English completion coming out as
    /// Cyrillic). Switch to `lang` first — a no-op when already active — exactly
    /// as [`Corrector::convert`] does before it retypes.
    pub fn insert_completion(&mut self, remainder: &str, lang: Lang) -> Result<()> {
        if self.layout.current().ok().flatten() != Some(lang) {
            self.layout.switch_to(lang)?;
        }
        self.injector.type_text(remainder)
    }

    /// Type a single trailing space (layout-independent) after a manual
    /// conversion, so the hotkey separates the word without a second keypress.
    pub fn append_space(&mut self) -> Result<()> {
        self.injector.type_text(" ")
    }

    /// Replay a real Tab keypress (stale-accept fallback: the key was swallowed
    /// but there is no suggestion to complete, so give the app its Tab back).
    pub fn tab(&mut self) -> Result<()> {
        self.injector.tab()
    }
}
