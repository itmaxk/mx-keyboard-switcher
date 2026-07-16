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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Toggle full auto (correction + suggestions) inside terminals.
    ToggleTerminalAuto,
    /// Arm "press a key" capture to reassign the autocomplete accept key.
    SetAcceptKey,
    /// Toggle the OS "start at login" entry.
    ToggleAutostart,
    /// Quit the application.
    Quit,
}

/// How the switcher behaves in the focused application (see `[exclusions]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMode {
    /// Automatic correction + suggestions and the manual hotkey all work.
    Full,
    /// Automatic correction and suggestions are suppressed, but the manual
    /// conversion hotkey still works on demand (terminals).
    ManualOnly,
    /// The switcher does nothing here, not even the hotkey (password managers).
    Off,
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
    /// Whether terminals get full auto (vs manual-only).
    pub terminal_auto: bool,
    /// Current accept key, human-readable (e.g. "Tab").
    pub accept_key: String,
    /// Whether an OS "start at login" entry exists (source of truth is the OS,
    /// not the config file).
    pub autostart: bool,
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
    /// The accept-key name currently applied to the intercept, so we only push a
    /// new spec when it actually changes (terminals may use a different key).
    active_accept: Option<String>,
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
            active_accept: None,
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
            terminal_auto: self.config.terminals.auto,
            accept_key: self.config.autocomplete.accept_key.clone(),
            autostart: crate::autostart::is_enabled(),
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
                // The global accept key is now applied; keep the tracker in sync.
                self.active_accept = Some(shown.clone());
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
        // Suggestions are automatic, so only where the app is fully enabled.
        let (mode, is_terminal) = self.classify_focus(&self.focus.focused_app());
        let want = if mode == AppMode::Full {
            self.compute_suggestion()
        } else {
            None
        };
        if want.is_none() && self.suggestion.is_none() {
            return;
        }
        match &want {
            // Re-send even when unchanged so the hint follows the caret.
            Some(text) => {
                // Pick the accept key for this app: terminals may use a separate
                // one so the global Tab does not hijack shell completion.
                let key = if is_terminal {
                    self.config.terminals.accept_key.clone()
                } else {
                    self.config.autocomplete.accept_key.clone()
                };
                self.set_accept_key(&key);
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
        if let Err(e) = self.corrector.insert_completion(&remainder, lang) {
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
        // Automatic correction runs only in fully-enabled apps.
        if self.app_mode() != AppMode::Full {
            return;
        }
        // Skip mixed-case identifiers (camelCase/PascalCase like "myVar",
        // "GitHub"): an interior capital marks a code token, not prose, so
        // auto-correcting it is almost always wrong. All-caps and a lone leading
        // capital are left alone by this check and still correct normally.
        let interior_upper = word.keys.iter().skip(1).any(|s| s.shift);
        let has_lower = word.keys.iter().any(|s| !s.shift);
        if interior_upper && has_lower {
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
        // The hotkey is on-demand, so it works everywhere except hard-off apps
        // (password managers). It intentionally still works in manual-only apps
        // like terminals.
        if self.app_mode() == AppMode::Off {
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
        // `in_progress` means no separator is on screen yet, so we add a trailing
        // space ourselves (the word already has its space in the last-completed
        // case, and convert() re-emits it).
        let (keys, from, trailing, in_progress) = if let Some(word) = self.buffer.current() {
            (word.keys, word.lang, String::new(), true)
        } else if let Some((word, trailing)) = self.last.take() {
            (word.keys, word.lang, trailing, false)
        } else {
            return;
        };

        let to = from.other();
        match self.corrector.convert(&keys, from, to, &trailing) {
            Ok(()) => {
                // Convert AND separate the in-progress word with a space, so the
                // user doesn't have to press Space right after the hotkey.
                let toggle_trailing = if in_progress {
                    if let Err(e) = self.corrector.append_space() {
                        tracing::warn!("append space failed: {e:#}");
                    }
                    " ".to_string()
                } else {
                    trailing
                };
                self.active = to;
                self.buffer.clear();
                self.buffer.set_lang(self.active);
                self.toggle = Some(Toggle {
                    keys,
                    trailing: toggle_trailing,
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
                // A visible suggestion may now be stale.
                self.dismiss_suggestion();
                // Re-apply the convert hotkey (the capture backend reads it live,
                // so this takes effect without a restart).
                if let Some(spec) = mxks_core::hotkey::parse(&self.config.hotkeys.convert_last_word)
                {
                    self.hotkey.set_spec(spec);
                }
                // Force the accept key (global vs terminal) to be re-resolved on
                // the next suggestion, since it may have changed in the config.
                self.active_accept = None;
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
            Command::ToggleTerminalAuto => {
                let on = !self.config.terminals.auto;
                self.config.terminals.auto = on;
                // A hint may be showing in a terminal that is now manual-only.
                self.dismiss_suggestion();
                self.active_accept = None;
                if let Err(e) = crate::config_io::save_terminal_auto(on) {
                    tracing::warn!("could not save terminal-auto switch: {e:#}");
                }
                tracing::info!("terminal auto = {on}");
                self.broadcast_status();
            }
            Command::SetAcceptKey => {
                self.capturing = true;
                self.hotkey.begin_capture(CaptureTarget::AcceptKey);
                tracing::info!("press a key (optionally with modifiers) to set the accept key");
                self.broadcast_status();
            }
            Command::ToggleAutostart => {
                let on = !crate::autostart::is_enabled();
                if let Err(e) = crate::autostart::set_enabled(on) {
                    tracing::warn!("could not update autostart: {e:#}");
                }
                tracing::info!("autostart = {on}");
                self.broadcast_status();
            }
            Command::Quit => return true,
        }
        false
    }

    /// Resolve how the switcher behaves in `app` and whether it is a terminal.
    /// Terminals are manual-only unless `[terminals] auto` is on; they also use a
    /// separate accept key, hence the returned `is_terminal` flag.
    fn classify_focus(&self, app: &Option<String>) -> (AppMode, bool) {
        let ex = &self.config.exclusions;
        let terms = &self.config.terminals;
        let Some(app) = app else {
            return (AppMode::Full, false);
        };
        let matches = |list: &[String]| {
            list.iter()
                .any(|e| !e.is_empty() && app.contains(&e.to_lowercase()))
        };
        // Hard exclusion (Off) wins over everything.
        if matches(&ex.apps) {
            (AppMode::Off, false)
        } else if matches(&terms.apps) {
            let mode = if terms.auto {
                AppMode::Full
            } else {
                AppMode::ManualOnly
            };
            (mode, true)
        } else if matches(&ex.manual_only) {
            (AppMode::ManualOnly, false)
        } else {
            (AppMode::Full, false)
        }
    }

    /// How the switcher should behave in the currently focused application.
    fn app_mode(&self) -> AppMode {
        self.classify_focus(&self.focus.focused_app()).0
    }

    /// Apply `name` as the intercept's accept key, but only when it changed
    /// (each change re-grabs the key in the backend).
    fn set_accept_key(&mut self, name: &str) {
        if self.active_accept.as_deref() == Some(name) {
            return;
        }
        if let Some(spec) = mxks_core::hotkey::parse(name) {
            if let Some(i) = &self.intercept {
                i.set_spec(spec);
            }
            self.active_accept = Some(name.to_string());
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
        current: Arc<Mutex<Lang>>,
    }
    impl LayoutSwitcher for MockLayout {
        fn current(&self) -> Result<Option<Lang>> {
            Ok(Some(*self.current.lock().unwrap()))
        }
        fn switch_to(&mut self, lang: Lang) -> Result<()> {
            self.log.lock().unwrap().push(Op::Switch(lang));
            *self.current.lock().unwrap() = lang;
            Ok(())
        }
    }

    struct MockFocus;
    impl FocusInfo for MockFocus {}

    /// Focus mock that reports a fixed application name (as `focused_app` would
    /// return a lowercased WM_CLASS), to exercise the per-app modes.
    struct MockFocusApp(String);
    impl FocusInfo for MockFocusApp {
        fn focused_app(&self) -> Option<String> {
            Some(self.0.clone())
        }
    }

    fn letter(key: PhysKey) -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Letter { key, shift: false },
            down: true,
            injected: false,
        }
    }
    fn letter_shift(key: PhysKey) -> KeyEvent {
        KeyEvent {
            kind: KeyKind::Letter { key, shift: true },
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
        let (engine, overlay_rx, icontrol, _layout) =
            autocomplete_engine_with_layout(log, Lang::En);
        (engine, overlay_rx, icontrol)
    }

    /// Like [`autocomplete_engine`], but starts the mock system layout at
    /// `start` and hands back the shared layout cell so a test can flip the
    /// active layout mid-word (to exercise injection-group correctness).
    fn autocomplete_engine_with_layout(
        log: Log,
        start: Lang,
    ) -> (
        Engine,
        crossbeam_channel::Receiver<OverlayCmd>,
        mxks_platform::InterceptControl,
        Arc<Mutex<Lang>>,
    ) {
        let current = Arc::new(Mutex::new(start));
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log,
                current: current.clone(),
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let (icontrol, ihandle) = mxks_platform::intercept_channel(mxks_platform::default_accept());
        let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded();
        let engine = Engine::new(Config::default(), corrector, Box::new(MockFocus), hk_handle)
            .with_autocomplete(overlay_tx, ihandle, true);
        (engine, overlay_rx, icontrol, current)
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
                current: Arc::new(Mutex::new(Lang::En)),
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
                Op::Switch(Lang::Ru),
                Op::Backspaces(7),
                Op::Type("привет".to_string()),
                Op::Type(" ".to_string()),
            ]
        );
    }

    #[test]
    fn layout_change_during_word_skips_autocorrect() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _overlay_rx, _icontrol, current) =
            autocomplete_engine_with_layout(log.clone(), Lang::En);
        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        key_tx.send(letter_shift(PhysKey::H)).unwrap();
        *current.lock().unwrap() = Lang::Ru;
        for key in [
            PhysKey::T,
            PhysKey::Comma,
            PhysKey::Backtick,
            PhysKey::Y,
            PhysKey::J,
            PhysKey::R,
        ] {
            key_tx.send(letter(key)).unwrap();
        }
        key_tx.send(space()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);
        assert!(log.lock().unwrap().is_empty());
    }

    /// Build an engine whose focus reports `app`, with a custom config, for the
    /// per-app mode tests. Returns the hotkey control side too so the caller
    /// keeps it alive (a dropped control disconnects the updates channel and
    /// makes `run`'s select! busy-spin).
    fn mode_engine(config: Config, app: &str, log: Log) -> (Engine, mxks_platform::HotkeyControl) {
        let corrector = Corrector::new(
            Box::new(MockInjector(log.clone())),
            Box::new(MockLayout {
                log,
                current: Arc::new(Mutex::new(Lang::En)),
            }),
        );
        let (hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let engine = Engine::new(
            config,
            corrector,
            Box::new(MockFocusApp(app.into())),
            hk_handle,
        );
        (engine, hk_ctrl)
    }

    /// The exact op sequence a manual `ghbdtn ` -> `привет` conversion produces.
    fn expected_ghbdtn_convert() -> Vec<Op> {
        vec![
            Op::Switch(Lang::Ru),
            Op::Backspaces(7),
            Op::Type("привет".to_string()),
            Op::Type(" ".to_string()),
        ]
    }

    fn ghbdtn() -> [KeyEvent; 6] {
        [
            letter(PhysKey::G),
            letter(PhysKey::H),
            letter(PhysKey::B),
            letter(PhysKey::D),
            letter(PhysKey::T),
            letter(PhysKey::N),
        ]
    }

    /// A manual-only app (via `exclusions.manual_only`): a Space boundary must
    /// NOT auto-correct, but the conversion hotkey still converts on demand.
    #[test]
    fn manual_only_app_skips_auto_but_allows_hotkey() {
        let mut config = Config::default();
        // Use a non-terminal name so this tests the manual_only list itself.
        config.exclusions.manual_only = vec!["mymanualapp".into()];
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _hk) = mode_engine(config, "mymanualapp mymanualapp", log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for ev in ghbdtn() {
            key_tx.send(ev).unwrap();
        }
        key_tx.send(space()).unwrap(); // must NOT auto-correct here
        key_tx.send(hotkey()).unwrap(); // manual convert must still work
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert_eq!(
            log.lock().unwrap().clone(),
            expected_ghbdtn_convert(),
            "manual-only app auto-corrected (should only convert on the hotkey)"
        );
    }

    /// A hard-excluded app (password manager): nothing runs, not even the
    /// conversion hotkey.
    #[test]
    fn hard_excluded_app_blocks_even_hotkey() {
        let mut config = Config::default();
        config.exclusions.apps = vec!["keepassxc".into()];
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _hk) = mode_engine(config, "keepassxc keepassxc", log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for ev in ghbdtn() {
            key_tx.send(ev).unwrap();
        }
        key_tx.send(space()).unwrap();
        key_tx.send(hotkey()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert!(
            log.lock().unwrap().is_empty(),
            "hard-excluded app must not convert, even via the hotkey"
        );
    }

    /// A full (unlisted) app still auto-corrects at a Space boundary.
    #[test]
    fn full_app_autocorrects() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _hk) = mode_engine(Config::default(), "org.some.editor", log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        for ev in ghbdtn() {
            key_tx.send(ev).unwrap();
        }
        key_tx.send(space()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert_eq!(log.lock().unwrap().clone(), expected_ghbdtn_convert());
    }

    /// Autocomplete-wired engine with a custom config and focused app name.
    fn autocomplete_engine_cfg(
        config: Config,
        app: &str,
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
                current: Arc::new(Mutex::new(Lang::En)),
            }),
        );
        let (_hk_ctrl, hk_handle) =
            mxks_platform::hotkey_channel(mxks_core::hotkey::HotkeySpec::default());
        let (icontrol, ihandle) = mxks_platform::intercept_channel(mxks_platform::default_accept());
        let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded();
        let engine = Engine::new(
            config,
            corrector,
            Box::new(MockFocusApp(app.into())),
            hk_handle,
        )
        .with_autocomplete(overlay_tx, ihandle, true);
        (engine, overlay_rx, icontrol)
    }

    /// A terminal defaults to manual-only: typing shows no suggestion and does
    /// not grab an accept key, but the conversion hotkey still converts.
    #[test]
    fn terminal_manual_only_by_default() {
        let mut config = Config::default();
        config.terminals.apps = vec!["myterm".into()];
        config.terminals.auto = false;
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _rx, icontrol) =
            autocomplete_engine_cfg(config, "myterm myterm", log.clone());

        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            engine.handle_key(letter(k));
        }
        assert!(
            !icontrol.is_active(),
            "a manual-only terminal must not intercept/suggest"
        );

        engine.handle_key(hotkey());
        assert!(
            !log.lock().unwrap().is_empty(),
            "the conversion hotkey must still work in a terminal"
        );
    }

    /// A terminal with `auto = true` suggests like a full app, but grabs its own
    /// accept key (Right) instead of the global one (Tab), so shell Tab stays free.
    #[test]
    fn terminal_auto_uses_its_own_accept_key() {
        let mut config = Config::default();
        config.terminals.apps = vec!["myterm".into()];
        config.terminals.auto = true;
        config.terminals.accept_key = "Right".into();
        config.autocomplete.accept_key = "Tab".into();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _rx, icontrol) = autocomplete_engine_cfg(config, "myterm myterm", log);

        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            engine.handle_key(letter(k));
        }
        assert!(
            icontrol.is_active(),
            "terminal auto should show a suggestion"
        );
        assert_eq!(
            icontrol.current().key,
            "RIGHT",
            "a terminal must grab its own accept key, not the global Tab"
        );
    }

    /// A full (non-terminal) app grabs the global accept key (Tab).
    #[test]
    fn full_app_uses_global_accept_key() {
        let mut config = Config::default();
        config.autocomplete.accept_key = "Tab".into();
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _rx, icontrol) = autocomplete_engine_cfg(config, "org.some.editor", log);

        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            engine.handle_key(letter(k));
        }
        assert!(icontrol.is_active(), "full app should show a suggestion");
        assert_eq!(icontrol.current().key, "TAB");
    }

    /// A mixed-case identifier (interior capital) must not be auto-corrected,
    /// even in a full app: "ghBdtn" would otherwise convert like "ghbdtn".
    #[test]
    fn mixed_case_identifier_is_not_autocorrected() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _hk) = mode_engine(Config::default(), "org.some.editor", log.clone());

        let (key_tx, key_rx) = crossbeam_channel::unbounded();
        let (_cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        // g h B d t n  (interior capital B) + space
        for ev in [
            letter(PhysKey::G),
            letter(PhysKey::H),
            letter_shift(PhysKey::B),
            letter(PhysKey::D),
            letter(PhysKey::T),
            letter(PhysKey::N),
        ] {
            key_tx.send(ev).unwrap();
        }
        key_tx.send(space()).unwrap();
        drop(key_tx);
        engine.run(key_rx, cmd_rx);

        assert!(
            log.lock().unwrap().is_empty(),
            "mixed-case identifier was auto-corrected"
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
                current: Arc::new(Mutex::new(Lang::En)),
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
                current: Arc::new(Mutex::new(Lang::En)),
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
                Op::Switch(Lang::Ru),
                Op::Backspaces(6),
                Op::Type("привет".to_string()),
                Op::Type(" ".to_string()),
                // Second press: toggle back RU -> EN, keeping the space (now on
                Op::Switch(Lang::En),
                Op::Backspaces(7),
                Op::Type("ghbdtn".to_string()),
                Op::Type(" ".to_string()),
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

    /// Regression: if the system layout drifts away from the word's language
    /// before Accept (e.g. the user switched layout mid-word), injection must
    /// force the group back to the completion's language first. The X11 injector
    /// replays physical key positions, so without this an English completion is
    /// typed through the Russian group and comes out as Cyrillic garbage.
    #[test]
    fn accept_forces_group_to_word_language() {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let (mut engine, _overlay_rx, _icontrol, layout) =
            autocomplete_engine_with_layout(log.clone(), Lang::En);

        // Type "hel" with English active: the word captures lang = En.
        for k in [PhysKey::H, PhysKey::E, PhysKey::L] {
            engine.handle_key(letter(k));
        }
        // The system layout flips to Russian before the accept key fires.
        *layout.lock().unwrap() = Lang::Ru;
        engine.handle_key(accept());

        // On accept the engine must switch back to En *before* typing.
        let ops = log.lock().unwrap().clone();
        assert_eq!(
            ops,
            vec![Op::Switch(Lang::En), Op::Type(expected_remainder("hel"))],
            "completion must be injected in the word's own layout"
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
