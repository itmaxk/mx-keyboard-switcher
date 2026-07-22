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
        let source_text = render_keys(keys, from);
        let replacement_text = render_keys(keys, to);
        tracing::info!(
            transaction_id,
            from = ?from,
            to = ?to,
            source_text = %source_text,
            replacement_text = %replacement_text,
            existing_trailing = %existing_trailing,
            replacement_trailing = %replacement_trailing,
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
        let target_already_active = match current {
            Some(active) if active == from => false,
            Some(active) if active == to => {
                tracing::info!(
                    transaction_id,
                    target_already_active = true,
                    verified_layout = ?to,
                    "conversion target already active; skipping layout switch"
                );
                true
            }
            _ => {
                let error = anyhow::anyhow!(
                    "layout changed before correction: expected source {from:?} or target {to:?}, got {current:?}"
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
        };

        if !target_already_active {
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
                let error = anyhow::anyhow!(
                    "target layout did not activate: expected {to:?}, got {current:?}"
                );
                tracing::error!(
                    transaction_id,
                    stage = "verify_target_layout",
                    actual = ?current,
                    error = %error,
                    "conversion failed"
                );
                return Err(error);
            }
        }
        let erase = source_text.chars().count() + existing_trailing.chars().count();
        tracing::info!(
            transaction_id,
            target_already_active,
            verified_layout = ?to,
            erase_chars = erase,
            replacement_chars = replacement_text.chars().count(),
            replacement_text = %replacement_text,
            replacement_trailing = %replacement_trailing,
            "conversion layout verified; injecting"
        );
        match self
            .injector
            .replace_text(erase, &replacement_text, replacement_trailing)
        {
            Ok(()) => {
                tracing::info!(
                    transaction_id,
                    injected_text = %replacement_text,
                    injected_trailing = %replacement_trailing,
                    "conversion succeeded"
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use mxks_core::keycode::PhysKey;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Op {
        Current,
        Switch(Lang),
        Replace {
            erase: usize,
            text: String,
            trailing: String,
        },
    }

    type Log = Arc<Mutex<Vec<Op>>>;

    struct MockInjector(Log);

    impl KeyInjector for MockInjector {
        fn backspaces(&mut self, _n: usize) -> Result<()> {
            unreachable!("convert must use atomic replace_text")
        }

        fn type_text(&mut self, _text: &str) -> Result<()> {
            unreachable!("convert must use atomic replace_text")
        }

        fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
            self.0.lock().unwrap().push(Op::Replace {
                erase,
                text: text.to_string(),
                trailing: trailing.to_string(),
            });
            Ok(())
        }
    }

    enum LayoutRead {
        Value(Option<Lang>),
        Failure,
    }

    struct MockLayout {
        log: Log,
        reads: Mutex<VecDeque<LayoutRead>>,
    }

    impl LayoutSwitcher for MockLayout {
        fn current(&self) -> Result<Option<Lang>> {
            self.log.lock().unwrap().push(Op::Current);
            match self.reads.lock().unwrap().pop_front() {
                Some(LayoutRead::Value(value)) => Ok(value),
                Some(LayoutRead::Failure) => bail!("layout read failed"),
                None => panic!("unexpected layout read"),
            }
        }

        fn switch_to(&mut self, lang: Lang) -> Result<()> {
            self.log.lock().unwrap().push(Op::Switch(lang));
            Ok(())
        }
    }

    fn corrector(reads: impl IntoIterator<Item = LayoutRead>) -> (Corrector, Log) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log: log.clone(),
                reads: Mutex::new(reads.into_iter().collect()),
            }),
        );
        (corrector, log)
    }

    fn how() -> [Stroke; 3] {
        [PhysKey::H, PhysKey::O, PhysKey::W].map(|key| Stroke { key, shift: false })
    }

    #[test]
    fn target_already_active_skips_switch_and_replaces_atomically() {
        let (mut corrector, log) = corrector([LayoutRead::Value(Some(Lang::Ru))]);

        corrector
            .convert(&how(), Lang::En, Lang::Ru, " ", " ")
            .unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                Op::Current,
                Op::Replace {
                    erase: 4,
                    text: "рщц".to_string(),
                    trailing: " ".to_string(),
                },
            ]
        );
    }

    #[test]
    fn source_active_switches_verifies_and_replaces_atomically() {
        let (mut corrector, log) = corrector([
            LayoutRead::Value(Some(Lang::En)),
            LayoutRead::Value(Some(Lang::Ru)),
        ]);

        corrector
            .convert(&how(), Lang::En, Lang::Ru, " ", " ")
            .unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                Op::Current,
                Op::Switch(Lang::Ru),
                Op::Current,
                Op::Replace {
                    erase: 4,
                    text: "рщц".to_string(),
                    trailing: " ".to_string(),
                },
            ]
        );
    }

    #[test]
    fn source_layout_read_failure_does_not_switch_or_inject() {
        let (mut corrector, log) = corrector([LayoutRead::Failure]);

        assert!(corrector
            .convert(&how(), Lang::En, Lang::Ru, " ", " ")
            .is_err());
        assert_eq!(*log.lock().unwrap(), vec![Op::Current]);
    }

    #[test]
    fn unknown_layout_does_not_switch_or_inject() {
        let (mut corrector, log) = corrector([LayoutRead::Value(None)]);

        assert!(corrector
            .convert(&how(), Lang::En, Lang::Ru, " ", " ")
            .is_err());
        assert_eq!(*log.lock().unwrap(), vec![Op::Current]);
    }

    #[test]
    fn requested_switch_still_requires_target_verification() {
        let (mut corrector, log) = corrector([
            LayoutRead::Value(Some(Lang::En)),
            LayoutRead::Value(Some(Lang::En)),
        ]);

        assert!(corrector
            .convert(&how(), Lang::En, Lang::Ru, " ", " ")
            .is_err());
        assert_eq!(
            *log.lock().unwrap(),
            vec![Op::Current, Op::Switch(Lang::Ru), Op::Current]
        );
    }
}
