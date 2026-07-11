//! `mxks-platform` — the OS abstraction boundary.
//!
//! The engine talks only to the traits declared here; each supported OS
//! provides a backend implementing them. Keeping the boundary small
//! ([`KeyCapture`], [`KeyInjector`], [`LayoutSwitcher`], [`FocusInfo`]) keeps
//! the platform-specific code isolated and the core testable.

pub mod event;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use mxks_core::hotkey::HotkeySpec;
use mxks_core::layout::Lang;

pub use event::{KeyEvent, KeyKind};

/// Captures global keyboard events and forwards them on `tx`. Runs its own
/// blocking loop on a dedicated thread; returns only on fatal error or shutdown.
pub trait KeyCapture: Send {
    fn run(&mut self, tx: Sender<KeyEvent>) -> Result<()>;
}

/// Synthesizes keyboard input. Injected events must be tagged so the capture
/// backend can mark them `injected` and the engine can ignore them.
pub trait KeyInjector: Send {
    /// Send `n` Backspace key presses.
    fn backspaces(&mut self, n: usize) -> Result<()>;
    /// Type `text` as Unicode input (layout-independent where the OS allows).
    fn type_text(&mut self, text: &str) -> Result<()>;
}

/// Reads and switches the active system keyboard layout.
pub trait LayoutSwitcher: Send {
    /// Best-effort current layout; `None` if it is neither EN nor RU.
    fn current(&self) -> Result<Option<Lang>>;
    /// Switch the active layout to `lang`.
    fn switch_to(&mut self, lang: Lang) -> Result<()>;
}

/// Best-effort information about the focused input, used to avoid correcting in
/// password fields. Implementations may always return `false`.
pub trait FocusInfo: Send {
    fn is_password_field(&self) -> bool {
        false
    }
    /// Lowercase process/app name of the focused window, if known.
    fn focused_app(&self) -> Option<String> {
        None
    }
}

/// A fully assembled platform backend.
pub struct Backend {
    pub capture: Box<dyn KeyCapture>,
    pub injector: Box<dyn KeyInjector>,
    pub layout: Box<dyn LayoutSwitcher>,
    pub focus: Box<dyn FocusInfo>,
    /// Live control over the conversion hotkey (reassign at runtime).
    pub hotkey: HotkeyHandle,
}

/// Shared with the capture backend: the current hotkey, a "capture the next key"
/// flag, and a channel to report a newly captured hotkey back to the app.
#[derive(Clone)]
pub struct HotkeyControl {
    spec: Arc<Mutex<HotkeySpec>>,
    capturing: Arc<AtomicBool>,
    updates: Sender<HotkeySpec>,
}

impl HotkeyControl {
    /// The currently active hotkey.
    pub fn current(&self) -> HotkeySpec {
        self.spec.lock().unwrap().clone()
    }
    /// True while waiting to capture the next keypress as the new hotkey.
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst)
    }
    /// Backend records a captured hotkey: update the live spec, stop capturing,
    /// and report it so the app can persist it.
    pub fn record(&self, spec: HotkeySpec) {
        *self.spec.lock().unwrap() = spec.clone();
        self.capturing.store(false, Ordering::SeqCst);
        let _ = self.updates.send(spec);
    }
}

/// App-side handle: start capture and receive the chosen hotkey.
pub struct HotkeyHandle {
    capturing: Arc<AtomicBool>,
    updates: Receiver<HotkeySpec>,
}

impl HotkeyHandle {
    /// Arm capture: the next keypress becomes the new hotkey.
    pub fn begin_capture(&self) {
        self.capturing.store(true, Ordering::SeqCst);
    }
    /// Channel of newly captured hotkeys (for the engine's select loop).
    pub fn updates(&self) -> &Receiver<HotkeySpec> {
        &self.updates
    }
}

/// Create a linked control/handle pair seeded with `initial`.
pub fn hotkey_channel(initial: HotkeySpec) -> (HotkeyControl, HotkeyHandle) {
    let capturing = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::unbounded();
    (
        HotkeyControl {
            spec: Arc::new(Mutex::new(initial)),
            capturing: capturing.clone(),
            updates: tx,
        },
        HotkeyHandle {
            capturing,
            updates: rx,
        },
    )
}

/// Build the backend for the current OS. The `hotkey` seeds the (reassignable)
/// conversion hotkey. Returns an error describing why the platform is
/// unsupported (e.g. a Wayland session on Linux).
pub fn backend(hotkey: HotkeySpec) -> Result<Backend> {
    imp::backend(hotkey)
}

// --- Per-OS backend selection ------------------------------------------------

#[cfg(target_os = "linux")]
#[path = "linux_x11/mod.rs"]
mod imp;

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod imp;

#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod imp;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod imp {
    use super::Backend;
    use anyhow::{bail, Result};
    use mxks_core::hotkey::HotkeySpec;
    pub fn backend(_hotkey: HotkeySpec) -> Result<Backend> {
        bail!("unsupported platform")
    }
}
