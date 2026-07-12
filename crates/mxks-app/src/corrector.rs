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

    /// Type text at the cursor (used to insert a completion remainder).
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        self.injector.type_text(text)
    }

    /// Replay a real Tab keypress (stale-accept fallback: the key was swallowed
    /// but there is no suggestion to complete, so give the app its Tab back).
    pub fn tab(&mut self) -> Result<()> {
        self.injector.tab()
    }
}
