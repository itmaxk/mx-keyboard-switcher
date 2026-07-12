//! The engine: consumes key events and tray commands, maintains the word
//! buffer, and drives detection + correction.

use crossbeam_channel::{Receiver, Sender};
use mxks_core::buffer::{Event, Word, WordBuffer};
use mxks_core::config::Config;
use mxks_core::convert::{render_keys, Stroke};
use mxks_core::detect::{analyze, Params, Verdict};
use mxks_core::layout::Lang;
use mxks_platform::{
    CaptureTarget, FocusInfo, HotkeyHandle, InterceptHandle, KeyEvent, KeyKind, OverlayCmd,
};

use crate::corrector::Corrector;

/// Commands sent from the tray (or other UI) to the engine.
// Some variants are only constructed by the tray, which is compiled out on
// platforms/builds without it.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Command {
    /// Toggle the master enable switch.
    ToggleEnabled,
    /// Toggle automatic correction only.
    ToggleAutocorrect,
    /// Open the config file in the default editor.
    OpenConfig,
    /// Reload config from disk (does not change the capture hotkey).
    ReloadConfig,
    /// Arm "press a key" capture to reassign the conversion hotkey.
    SetHotkey,
    /// Toggle word autocomplete.
    ToggleAutocomplete,
    /// Arm "press a key" capture to reassign the autocomplete accept key.
    SetAcceptKey,
    /// Quit the application.
    Quit,
}

/// Snapshot of engine state the tray can render.
// Fields are read only by the tray, which is compiled out on some builds.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Status {
    pub enabled: bool,
    pub autocorrect: bool,
    /// Current conversion hotkey, human-readable (e.g. "Pause", "Ctrl+Shift+K").
    pub hotkey: String,
    /// True while waiting for the user to press a key to assign.
    pub capturing: bool,
    /// Autocomplete on/off (false also when the platform has no overlay).
    pub autocomplete: bool,
    /// Current accept key, human-readable (e.g. "Tab").
    pub accept_key: String,
}

/// The result of the most recent *manual* conversion, kept so the hotkey can
/// toggle it back and forth as long as nothing else is typed.
struct Toggle {
    keys: Vec<Stroke>,
    trailing: String,
    /// The layout the text is currently displayed in.
    lang: Lang,
}

pub struct Engine {
    buffer: WordBuffer,
    config: Config,
    corrector: Corrector,
    focus: Box<dyn FocusInfo>,
    hotkey: HotkeyHandle,
    active: Lang,
    enabled: bool,
    capturing: bool,
    /// Last completed word + its trailing separator, for the manual hotkey.
    last: Option<(Word, String)>,
    /// State for toggling the last manual conversion back and forth.
    toggle: Option<Toggle>,
    /// Broadcasts status changes so the tray can update its menu.
    status_tx: Option<Sender<Status>>,
    /// Remainder of the currently suggested completion, if one is visible.
    suggestion: Option<String>,
    /// Channel to the overlay thread (non-blocking sends only).
    overlay_tx: Option<Sender<OverlayCmd>>,
    /// Toggles accept-key interception in the capture backend.
    intercept: Option<InterceptHandle>,
    /// False when the platform has no overlay — autocomplete stays inert.
    overlay_available: bool,
}

impl Engine {
    pub fn new(
        config: Config,
        corrector: Corrector,
        focus: Box<dyn FocusInfo>,
        hotkey: HotkeyHandle,
    ) -> Self {
        let active = corrector.current_layout().unwrap_or(Lang::En);
        Engine {
            buffer: WordBuffer::new(active),
            config,
            corrector,
            focus,
            hotkey,
            active,
            enabled: true,
            capturing: false,
            last: None,
            toggle: None,
            status_tx: None,
            suggestion: None,
            overlay_tx: None,
            intercept: None,
            overlay_available: false,
        }
    }

    pub fn with_status_channel(mut self, tx: Sender<Status>) -> Self {
        self.status_tx = Some(tx);
        self
    }

    /// Wire up autocomplete: the overlay command channel, the accept-key
    /// interception handle, and whether the platform overlay actually works.
    pub fn with_autocomplete(
        mut self,
        overlay_tx: Sender<OverlayCmd>,
        intercept: InterceptHandle,
        overlay_available: bool,
    ) -> Self {
        self.overlay_tx = Some(overlay_tx);
        self.intercept = Some(intercept);
        self.overlay_available = overlay_available;
        self
    }

    pub fn status(&self) -> Status {
        Status {
            enabled: self.enabled,
            autocorrect: self.config.general.autocorrect,
            hotkey: self.config.hotkeys.convert_last_word.clone(),
            capturing: self.capturing,
            autocomplete: self.config.autocomplete.enabled && self.overlay_available,
            accept_key: self.config.autocomplete.accept_key.clone(),
        }
    }

    fn broadcast_status(&self) {
        if let Some(tx) = &self.status_tx {
            let _ = tx.send(self.status());
        }
    }

    /// Main loop. Returns when either channel closes or `Quit` is received.
    pub fn run(&mut self, key_rx: Receiver<KeyEvent>, cmd_rx: Receiver<Command>) {
        self.broadcast_status();
        let hk_rx = self.hotkey.updates().clone();
        loop {
            crossbeam_channel::select! {
                recv(key_rx) -> msg => match msg {
                    Ok(ev) => self.handle_key(ev),
                    Err(_) => break,
                },
                recv(cmd_rx) -> msg => {
                    if let Ok(cmd) = msg {
                        if self.handle_command(cmd) { break; }
                    }
                },
                recv(hk_rx) -> msg => {
                    if let Ok((target, spec)) = msg {
                        self.on_key_assigned(target, spec);
                    }
                },
            }
        }
    }

    /// A new key was captured: persist it and update state.
    fn on_key_assigned(&mut self, target: CaptureTarget, spec: mxks_core::hotkey::HotkeySpec) {
        let shown = spec.display();
        self.capturing = false;
        match target {
            CaptureTarget::ConvertHotkey => {
                self.config.hotkeys.convert_last_word = shown.clone();
                if let Err(e) = crate::config_io::save_hotkey(&shown) {
                    tracing::warn!("could not save hotkey: {e:#}");
                }
                tracing::info!("conversion hotkey set to {shown}");
            }
            CaptureTarget::AcceptKey => {
                self.config.autocomplete.accept_key = shown.clone();
                if let Some(i) = &self.intercept {
                    i.set_spec(spec);
                }
                if let Err(e) = crate::config_io::save_accept_key(&shown) {
                    tracing::warn!("could not save accept key: {e:#}");
                }
                tracing::info!("autocomplete accept key set to {shown}");
            }
        }
        self.broadcast_status();
    }

    fn handle_key(&mut self, ev: KeyEvent) {
        if !ev.down || ev.injected {
            return;
        }
        // Any real typing invalidates the manual-conversion toggle.
        if !matches!(ev.kind, KeyKind::Hotkey) {
            self.toggle = None;
        }
        match ev.kind {
            KeyKind::Letter { key, shift } => {
                if self.buffer.is_empty() {
                    // Refresh active layout at word start to track user switches.
                    if let Some(l) = self.corrector.current_layout() {
                        self.active = l;
                        self.buffer.set_lang(l);
                    }
                    self.last = None;
                }
                self.buffer.feed(Event::Letter(Stroke { key, shift }));
                self.refresh_suggestion();
            }
            KeyKind::Backspace => {
                self.buffer.feed(Event::Backspace);
                self.refresh_suggestion();
            }
            KeyKind::Boundary { sep } => {
                self.dismiss_suggestion();
                if let Some(word) = self.buffer.feed(Event::Boundary) {
                    self.on_word(word, sep);
                }
            }
            KeyKind::Reset => {
                self.dismiss_suggestion();
                self.buffer.feed(Event::Reset);
                self.last = None;
            }
            KeyKind::Hotkey => {
                self.dismiss_suggestion();
                self.manual_convert();
            }
            KeyKind::Accept => self.on_accept(),
        }
    }

    /// Recompute the completion for the in-progress word and sync the overlay
    /// and accept-key interception with it.
    fn refresh_suggestion(&mut self) {
        if self.overlay_tx.is_none() {
            return;
        }
        let want = self.compute_suggestion();
        if want.is_none() && self.suggestion.is_none() {
            return;
        }
        match &want {
            // Re-send even when unchanged so the hint follows the caret.
            Some(text) => {
                self.send_overlay(OverlayCmd::Show { text: text.clone() });
                self.set_intercept(true);
            }
            None => {
                self.send_overlay(OverlayCmd::Hide);
                self.set_intercept(false);
            }
        }
        self.suggestion = want;
    }

    /// The remainder to suggest for the current word, if any.
    fn compute_suggestion(&self) -> Option<String> {
        let ac = &self.config.autocomplete;
        if !ac.enabled || !self.enabled || !self.overlay_available {
            return None;
        }
        let word = self.buffer.current()?;
        let prefix = render_keys(&word.keys, word.lang).to_lowercase();
        let prefix_len = prefix.chars().count();
        if prefix_len < ac.min_prefix {
            return None;
        }
        if self.app_excluded() {
            return None;
        }
        let full = mxks_core::dict::complete(&prefix, word.lang)?;
        let remainder: String = full.chars().skip(prefix_len).collect();
        if remainder.chars().count() < ac.min_remainder {
            return None;
        }
        // All-caps input gets an all-caps completion; otherwise keep lowercase
        // (a leading capital still yields "Hel" + "lo").
        if word.keys.len() >= 2 && word.keys.iter().all(|s| s.shift) {
            return Some(remainder.to_uppercase());
        }
        Some(remainder)
    }

    /// Hide the hint and stop intercepting the accept key.
    fn dismiss_suggestion(&mut self) {
        if self.suggestion.take().is_some() {
            self.send_overlay(OverlayCmd::Hide);
            self.set_intercept(false);
        }
    }

    /// The accept key fired (and was swallowed by the backend where possible).
    fn on_accept(&mut self) {
        let Some(remainder) = self.suggestion.take() else {
            // Stale accept: the suggestion was dismissed while the keypress was
            // in flight. Replay a real Tab so the user's keystroke isn't lost
            // (other accept keys are inert on their own — just drop those).
            self.set_intercept(false);
            if self
                .intercept
                .as_ref()
                .is_some_and(|i| i.current().key == "TAB")
            {
                if let Err(e) = self.corrector.tab() {
                    tracing::warn!("could not replay tab: {e:#}");
                }
                if let Some(word) = self.buffer.feed(Event::Boundary) {
                    self.on_word(word, None);
                }
            } else {
                self.buffer.feed(Event::Reset);
            }
            return;
        };

        self.send_overlay(OverlayCmd::Hide);
        self.set_intercept(false);

        let lang = self.buffer.current().map(|w| w.lang).unwrap_or(self.active);
        if let Err(e) = self.corrector.type_text(&remainder) {
            tracing::warn!("completion injection failed: {e:#}");
            return;
        }
        // Mirror the injected letters into the buffer so it matches the screen;
        // the completed word is a dictionary word of `lang` by construction, so
        // a later Space boundary will never "correct" it.
        for c in remainder.chars() {
            let lower = mxks_core::layout::to_lower(c);
            if let Some(key) = mxks_core::layout::char_to_key(lower, lang) {
                self.buffer.feed(Event::Letter(Stroke {
                    key,
                    shift: c != lower,
                }));
            }
        }
    }

    fn send_overlay(&self, cmd: OverlayCmd) {
        if let Some(tx) = &self.overlay_tx {
            let _ = tx.send(cmd);
        }
    }

    fn set_intercept(&self, on: bool) {
        if let Some(i) = &self.intercept {
            i.set_active(on);
        }
    }

    fn on_word(&mut self, word: Word, sep: Option<char>) {
        let trailing = sep.map(|c| c.to_string()).unwrap_or_default();
        self.last = Some((word.clone(), trailing.clone()));

        if !self.enabled || !self.config.general.autocorrect {
            return;
        }
        // v1 only auto-corrects at a Space boundary.
        if sep.is_none() {
            return;
        }
        if self.app_excluded() {
            return;
        }

        let verdict = {
            let params = self.params();
            analyze(&word, &params)
        };
        if tracing::enabled!(tracing::Level::DEBUG) {
            let typed = mxks_core::convert::render_keys(&word.keys, word.lang);
            let converted = mxks_core::convert::render_keys(&word.keys, word.lang.other());
            tracing::debug!(
                "word: active={:?} typed={:?} converted={:?} verdict={:?}",
                word.lang,
                typed,
                converted,
                verdict
            );
        }
        if let Verdict::Correct(conv) = verdict {
            let to = word.lang.other();
            match self.corrector.convert(&word.keys, word.lang, to, &trailing) {
                Ok(()) => {
                    self.active = to;
                    self.last = None;
                    tracing::debug!("corrected -> {conv}");
                }
                Err(e) => tracing::warn!("autocorrect failed: {e:#}"),
            }
        }
    }

    /// The conversion hotkey. First press converts the current (or last) word
    /// and switches layout; pressing it again with nothing typed in between
    /// toggles the same word back and forth.
    fn manual_convert(&mut self) {
        if !self.enabled {
            return;
        }

        // Toggle the previous manual conversion back and forth.
        if let Some(t) = self.toggle.take() {
            let to = t.lang.other();
            match self.corrector.convert(&t.keys, t.lang, to, &t.trailing) {
                Ok(()) => {
                    self.active = to;
                    self.buffer.set_lang(self.active);
                    self.toggle = Some(Toggle {
                        keys: t.keys,
                        trailing: t.trailing,
                        lang: to,
                    });
                }
                Err(e) => {
                    tracing::warn!("toggle convert failed: {e:#}");
                    self.toggle = Some(t);
                }
            }
            return;
        }

        // First conversion: prefer the in-progress word, else the last completed.
        let (keys, from, trailing) = if let Some(word) = self.buffer.current() {
            (word.keys, word.lang, String::new())
        } else if let Some((word, trailing)) = self.last.take() {
            (word.keys, word.lang, trailing)
        } else {
            return;
        };

        let to = from.other();
        match self.corrector.convert(&keys, from, to, &trailing) {
            Ok(()) => {
                self.active = to;
                self.buffer.clear();
                self.buffer.set_lang(self.active);
                self.toggle = Some(Toggle {
                    keys,
                    trailing,
                    lang: to,
                });
            }
            Err(e) => tracing::warn!("manual convert failed: {e:#}"),
        }
    }

    fn handle_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::ToggleEnabled => {
                self.enabled = !self.enabled;
                tracing::info!("enabled = {}", self.enabled);
                self.broadcast_status();
            }
            Command::ToggleAutocorrect => {
                self.config.general.autocorrect = !self.config.general.autocorrect;
                tracing::info!("autocorrect = {}", self.config.general.autocorrect);
                self.broadcast_status();
            }
            Command::ReloadConfig => {
                self.config = crate::config_io::load();
                // Re-apply autocomplete settings: the accept key may have
                // changed and a visible suggestion may now be stale.
                self.dismiss_suggestion();
                if let Some(spec) = mxks_core::hotkey::parse(&self.config.autocomplete.accept_key) {
                    if let Some(i) = &self.intercept {
                        i.set_spec(spec);
                    }
                }
                tracing::info!("config reloaded");
                self.broadcast_status();
            }
            Command::OpenConfig => {
                if let Ok(path) = crate::config_io::config_path() {
                    crate::open_path(&path);
                }
            }
            Command::SetHotkey => {
                self.capturing = true;
                self.hotkey.begin_capture(CaptureTarget::ConvertHotkey);
                tracing::info!("press a key (optionally with modifiers) to set the hotkey");
                self.broadcast_status();
            }
            Command::ToggleAutocomplete => {
                let on = !self.config.autocomplete.enabled;
                self.config.autocomplete.enabled = on;
                if !on {
                    self.dismiss_suggestion();
                }
                if let Err(e) = crate::config_io::save_autocomplete_enabled(on) {
                    tracing::warn!("could not save autocomplete switch: {e:#}");
                }
                tracing::info!("autocomplete = {on}");
                self.broadcast_status();
            }
            Command::SetAcceptKey => {
                self.capturing = true;
                self.hotkey.begin_capture(CaptureTarget::AcceptKey);
                tracing::info!("press a key (optionally with modifiers) to set the accept key");
                self.broadcast_status();
            }
            Command::Quit => return true,
        }
        false
    }

    fn app_excluded(&self) -> bool {
        if self.config.exclusions.apps.is_empty() {
            return false;
        }
        match self.focus.focused_app() {
            Some(app) => self
                .config
                .exclusions
                .apps
                .iter()
                .any(|ex| !ex.is_empty() && app.contains(&ex.to_lowercase())),
            None => false,
        }
    }

    fn params(&self) -> Params<'_> {
        Params {
            min_word_len: self.config.general.min_word_len,
            threshold: self.config.detection.threshold,
            extra_en: &self.config.dictionary.extra_en,
            extra_ru: &self.config.dictionary.extra_ru,
            never_words: &self.config.exclusions.words,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use mxks_core::keycode::PhysKey;
    use mxks_platform::{KeyInjector, LayoutSwitcher};
    use std::sync::{Arc, Mutex};

    /// A single recorded output operation, in execution order.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Op {
        Backspaces(usize),
        Switch(Lang),
        Type(String),
        Tab,
    }

    type Log = Arc<Mutex<Vec<Op>>>;

    struct MockInjector(Log);
    impl KeyInjector for MockInjector {
        fn backspaces(&mut self, n: usize) -> Result<()> {
            self.0.lock().unwrap().push(Op::Backspaces(n));
            Ok(())
        }
        fn type_text(&mut self, text: &str) -> Result<()> {
            self.0.lock().unwrap().push(Op::Type(text.to_string()));
            Ok(())
        }
        fn tab(&mut self) -> Result<()> {
            self.0.lock().unwrap().push(Op::Tab);
            Ok(())
        }
    }

    struct MockLayout {
        log: Log,
        current: Lang,
    }
    impl LayoutSwitcher for MockLayout {
        fn current(&self) -> Result<Option<Lang>> {
            Ok(Some(self.current))
        }
        fn switch_to(&mut self, lang: Lang) -> Result<()> {
            self.log.lock().unwrap().push(Op::Switch(lang));
            Ok(())
        }
    }

    struct MockFocus;
    impl FocusInfo for MockFocus {}

    fn letter(key: PhysKey) -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Letter { key, shift: false },
            down: true,
            injected: false,
        }
    }
    fn space() -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Boundary { sep: Some(' ') },
            down: true,
            injected: false,
        }
    }
    fn hotkey() -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Hotkey,
            down: true,
            injected: false,
        }
    }
    fn accept() -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Accept,
            down: true,
            injected: false,
        }
    }
    fn backspace() -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Backspace,
            down: true,
            injected: false,
        }
    }

    /// Engine with autocomplete wired to a mock overlay channel and a real
    /// intercept pair (the control side lets tests observe the active flag).
    fn autocomplete_engine(
        log: Log,
    ) -> (
        Engine,
        crossbeam_channel::Receiver<OverlayCmd>,
        mxks_platform::InterceptControl,
    ) {
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log,
                current: Lang::En,
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let (icontrol, ihandle) = mxks_platform::intercept_channel(mxks_platform::default_accept());
        let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded();
        let engine = Engine::new(Config::default(), corrector, Box::new(MockFocus), hk_handle)
            .with_autocomplete(overlay_tx, ihandle, true);
        (engine, overlay_rx, icontrol)
    }

    /// The expected completion remainder for an English prefix, straight from
    /// the dictionary (keeps tests independent of frequency-table contents).
    fn expected_remainder(prefix: &str) -> String {
        let full = mxks_core::dict::complete(prefix, Lang::En)
            .unwrap_or_else(|| panic!("dictionary has no completion for {prefix:?}"));
        full.chars().skip(prefix.chars().count()).collect()
    }

    /// Typing "ghbdtn " in English must produce the exact correction sequence:
    /// erase 6 letters + the space, switch to Russian, retype "привет" + " ".
    #[test]
    fn autocorrects_wrong_layout_word() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log: log.clone(),
                current: Lang::En,
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let mut engine = Engine::new(Config::default(), corrector, Box::new(MockFocus), hk_handle);

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for k in [
            PhysKey::G,
            PhysKey::H,
            PhysKey::B,
            PhysKey::D,
            PhysKey::T,
            PhysKey::N,
        ] {
            key_tx.send(letter(k)).unwrap();
        }
        key_tx.send(space()).unwrap();
        drop(key_tx); // closing the channel ends the run loop after draining

        engine.run(key_rx, cmd_rx);

        let ops = log.lock().unwrap().clone();
        assert_eq!(
            ops,
            vec![
                Op::Backspaces(7),
                Op::Switch(Lang::Ru),
                Op::Type("привет".to_string()),
                Op::Type(" ".to_string()),
            ]
        );
    }

    /// A valid English word must not be touched.
    #[test]
    fn leaves_valid_word_untouched() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log: log.clone(),
                current: Lang::En,
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let mut engine = Engine::new(Config::default(), corrector, Box::new(MockFocus), hk_handle);

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        // "hello"
        for k in [PhysKey::H, PhysKey::E, PhysKey::L, PhysKey::L, PhysKey::O] {
            key_tx.send(letter(k)).unwrap();
        }
        key_tx.send(space()).unwrap();
        drop(key_tx);

        engine.run(key_rx, cmd_rx);
        assert!(log.lock().unwrap().is_empty(), "valid word was modified");
    }

    /// Pressing the hotkey converts the in-progress word; pressing it again with
    /// nothing typed in between toggles it back.
    #[test]
    fn hotkey_toggles_conversion() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log: log.clone(),
                current: Lang::En,
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let mut engine = Engine::new(Config::default(), corrector, Box::new(MockFocus), hk_handle);

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        // Type "ghbdtn" (no space), then press the hotkey twice.
        for k in [
            PhysKey::G,
            PhysKey::H,
            PhysKey::B,
            PhysKey::D,
            PhysKey::T,
            PhysKey::N,
        ] {
            key_tx.send(letter(k)).unwrap();
        }
        key_tx.send(hotkey()).unwrap();
        key_tx.send(hotkey()).unwrap();
        drop(key_tx);

        engine.run(key_rx, cmd_rx);

        let ops = log.lock().unwrap().clone();
        assert_eq!(
            ops,
            vec![
                // First press: EN -> RU.
                Op::Backspaces(6),
                Op::Switch(Lang::Ru),
                Op::Type("привет".to_string()),
                // Second press: toggle back RU -> EN.
                Op::Backspaces(6),
                Op::Switch(Lang::En),
                Op::Type("ghbdtn".to_string()),
            ]
        );
    }

    /// Three letters ("hel") must produce a Show with the dictionary remainder
    /// and turn accept-key interception on.
    #[test]
    fn suggestion_appears_after_min_prefix() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, overlay_rx, icontrol) = autocomplete_engine(log);

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            key_tx.send(letter(k)).unwrap();
        }
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        let last = overlay_rx.try_iter().last().expect("no overlay commands");
        match last {
            OverlayCmd::Show { text } => assert_eq!(text, expected_remainder("hel")),
            other => panic!("expected Show, got {other:?}"),
        }
        assert!(icontrol.is_active(), "interception not enabled");
    }

    /// Accept must type exactly the remainder, and the following Space must not
    /// trigger any correction (the completed word is a dictionary word).
    #[test]
    fn accept_types_remainder_and_space_does_not_correct() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, overlay_rx, icontrol) = autocomplete_engine(log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            key_tx.send(letter(k)).unwrap();
        }
        key_tx.send(accept()).unwrap();
        key_tx.send(space()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        let ops = log.lock().unwrap().clone();
        assert_eq!(ops, vec![Op::Type(expected_remainder("hel"))]);
        assert!(!icontrol.is_active(), "interception left on after accept");
        assert!(
            matches!(overlay_rx.try_iter().last(), Some(OverlayCmd::Hide)),
            "overlay not hidden after accept"
        );
    }

    /// A stale Accept (no suggestion on screen) must replay a real Tab so the
    /// user's keystroke isn't lost.
    #[test]
    fn stale_accept_replays_tab() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _overlay_rx, _icontrol) = autocomplete_engine(log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        key_tx.send(accept()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert_eq!(log.lock().unwrap().clone(), vec![Op::Tab]);
    }

    /// Backspacing below min_prefix must hide the hint and drop interception.
    #[test]
    fn backspace_below_min_prefix_hides_suggestion() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, overlay_rx, icontrol) = autocomplete_engine(log);

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            key_tx.send(letter(k)).unwrap();
        }
        key_tx.send(backspace()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert!(
            matches!(overlay_rx.try_iter().last(), Some(OverlayCmd::Hide)),
            "overlay not hidden after backspace below min_prefix"
        );
        assert!(!icontrol.is_active(), "interception left on");
    }
}
