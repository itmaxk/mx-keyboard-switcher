//! The engine: consumes key events and tray commands, maintains the word
//! buffer, and drives detection + correction.

use crossbeam_channel::{Receiver, Sender};
use mxks_core::buffer::{Event, Word, WordBuffer};
use mxks_core::config::Config;
use mxks_core::convert::Stroke;
use mxks_core::detect::{analyze, Params, Verdict};
use mxks_core::layout::Lang;
use mxks_platform::{FocusInfo, HotkeyHandle, KeyEvent, KeyKind};

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
    /// Broadcasts status changes so the tray can update its menu.
    status_tx: Option<Sender<Status>>,
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
            status_tx: None,
        }
    }

    pub fn with_status_channel(mut self, tx: Sender<Status>) -> Self {
        self.status_tx = Some(tx);
        self
    }

    pub fn status(&self) -> Status {
        Status {
            enabled: self.enabled,
            autocorrect: self.config.general.autocorrect,
            hotkey: self.config.hotkeys.convert_last_word.clone(),
            capturing: self.capturing,
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
                    if let Ok(spec) = msg {
                        self.on_hotkey_assigned(spec);
                    }
                },
            }
        }
    }

    /// A new hotkey was captured: persist it and update state.
    fn on_hotkey_assigned(&mut self, spec: mxks_core::hotkey::HotkeySpec) {
        let shown = spec.display();
        self.config.hotkeys.convert_last_word = shown.clone();
        self.capturing = false;
        if let Err(e) = crate::config_io::save_hotkey(&shown) {
            tracing::warn!("could not save hotkey: {e:#}");
        }
        tracing::info!("conversion hotkey set to {shown}");
        self.broadcast_status();
    }

    fn handle_key(&mut self, ev: KeyEvent) {
        if !ev.down || ev.injected {
            return;
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
            }
            KeyKind::Backspace => {
                self.buffer.feed(Event::Backspace);
            }
            KeyKind::Boundary { sep } => {
                if let Some(word) = self.buffer.feed(Event::Boundary) {
                    self.on_word(word, sep);
                }
            }
            KeyKind::Reset => {
                self.buffer.feed(Event::Reset);
                self.last = None;
            }
            KeyKind::Hotkey => self.manual_convert(),
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
            match self.corrector.autocorrect(&word, &conv, &trailing) {
                Ok(()) => {
                    self.active = word.lang.other();
                    self.last = None;
                    tracing::debug!("corrected -> {conv}");
                }
                Err(e) => tracing::warn!("autocorrect failed: {e:#}"),
            }
        }
    }

    fn manual_convert(&mut self) {
        if !self.enabled {
            return;
        }
        // Prefer the in-progress word; otherwise the last completed word.
        if let Some(word) = self.buffer.current() {
            match self.corrector.manual(&word) {
                Ok(_) => {
                    self.active = word.lang.other();
                    self.buffer.clear();
                    self.buffer.set_lang(self.active);
                }
                Err(e) => tracing::warn!("manual convert failed: {e:#}"),
            }
        } else if let Some((word, trailing)) = self.last.take() {
            let converted = mxks_core::convert::render_keys(&word.keys, word.lang.other());
            match self.corrector.autocorrect(&word, &converted, &trailing) {
                Ok(()) => {
                    self.active = word.lang.other();
                    self.buffer.set_lang(self.active);
                }
                Err(e) => tracing::warn!("manual convert failed: {e:#}"),
            }
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
                self.hotkey.begin_capture();
                tracing::info!("press a key (optionally with modifiers) to set the hotkey");
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
}
