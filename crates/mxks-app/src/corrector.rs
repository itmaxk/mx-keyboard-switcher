//! Applies corrections: erase the currently displayed text, switch the system
//! layout, retype the text through the target layout. Owns the injector and
//! layout switcher.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use mxks_core::convert::{render_keys, Stroke};
use mxks_core::layout::Lang;
use mxks_platform::{KeyInjector, LayoutSwitcher};

static NEXT_CONVERSION_ID: AtomicU64 = AtomicU64::new(1);

pub struct Corrector {
    injector: Box<dyn KeyInjector>,
    layout: Box<dyn LayoutSwitcher>,
}

impl Corrector {
    pub fn new(injector: Box<dyn KeyInjector>, layout: Box<dyn LayoutSwitcher>) -> Self {
        Corrector { injector, layout }
    }

    /// Re-render `keys` from its current layout `from` into `to`: erase what is
    /// on screen (the `from` rendering plus `existing_trailing`), switch the
    /// system layout to `to`, and type the `to` rendering plus
    /// `replacement_trailing`.
    ///
    /// Erase length is computed from the actual rendered text (not the key
    /// count), so it is correct even when the two layouts render different
    /// character counts.
    pub fn convert(
        &mut self,
        keys: &[Stroke],
        from: Lang,
        to: Lang,
        existing_trailing: &str,
        replacement_trailing: &str,
    ) -> Result<()> {
        let transaction_id = NEXT_CONVERSION_ID.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            transaction_id,
            from = ?from,
            to = ?to,
            stroke_count = keys.len(),
            existing_trailing_chars = existing_trailing.chars().count(),
            replacement_trailing_chars = replacement_trailing.chars().count(),
            "conversion begin"
        );

        let current = match self.layout.current() {
            Ok(current) => current,
            Err(error) => {
                tracing::error!(
                    transaction_id,
                    stage = "read_source_layout",
                    error = %format_args!("{error:#}"),
                    "conversion failed"
                );
                return Err(error);
            }
        };
        if current != Some(from) {
            let error = anyhow::anyhow!(
                "source layout changed before correction: expected {from:?}, got {current:?}"
            );
            tracing::error!(
                transaction_id,
                stage = "verify_source_layout",
                actual = ?current,
                error = %error,
                "conversion failed"
            );
            return Err(error);
        }
        if let Err(error) = self.layout.switch_to(to) {
            tracing::error!(
                transaction_id,
                stage = "switch_layout",
                error = %format_args!("{error:#}"),
                "conversion failed"
            );
            return Err(error);
        }
        let current = match self.layout.current() {
            Ok(current) => current,
            Err(error) => {
                tracing::error!(
                    transaction_id,
                    stage = "read_target_layout",
                    error = %format_args!("{error:#}"),
                    "conversion failed"
                );
                return Err(error);
            }
        };
        if current != Some(to) {
            let error =
                anyhow::anyhow!("target layout did not activate: expected {to:?}, got {current:?}");
            tracing::error!(
                transaction_id,
                stage = "verify_target_layout",
                actual = ?current,
                error = %error,
                "conversion failed"
            );
            return Err(error);
        }
        let rendered = render_keys(keys, from);
        let erase = rendered.chars().count() + existing_trailing.chars().count();
        let text = render_keys(keys, to);
        tracing::info!(
            transaction_id,
            verified_layout = ?to,
            erase_chars = erase,
            replacement_chars = text.chars().count(),
            replacement_trailing_chars = replacement_trailing.chars().count(),
            "conversion layout verified; injecting"
        );
        match self
            .injector
            .replace_text(erase, &text, replacement_trailing)
        {
            Ok(()) => {
                tracing::info!(transaction_id, "conversion succeeded");
                Ok(())
            }
            Err(error) => {
                tracing::error!(
                    transaction_id,
                    stage = "replace_text",
                    error = %format_args!("{error:#}"),
                    "conversion failed"
                );
                Err(error)
            }
        }
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

    /// Replay a real Tab keypress (stale-accept fallback: the key was swallowed
    /// but there is no suggestion to complete, so give the app its Tab back).
    pub fn tab(&mut self) -> Result<()> {
        self.injector.tab()
    }
}
