//! `mxks-platform` — the OS abstraction boundary.
//!
//! The engine talks only to the traits declared here; each supported OS
//! provides a backend implementing them. Keeping the boundary small
//! ([`KeyCapture`], [`KeyInjector`], [`LayoutSwitcher`], [`FocusInfo`]) keeps
//! the platform-specific code isolated and the core testable.

pub mod event;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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
    /// Atomically replace erased text with rendered text and trailing separator.
    /// Platforms without a batch primitive use the default sequential path.
    fn replace_text(&mut self, erase: usize, text: &str, trailing: &str) -> Result<()> {
        self.backspaces(erase)?;
        self.type_text(text)?;
        if !trailing.is_empty() {
            self.type_text(trailing)?;
        }
        Ok(())
    }
    /// Send one real Tab keypress (replays a swallowed accept key that turned
    /// out to be stale).
    fn tab(&mut self) -> Result<()> {
        anyhow::bail!("tab injection not supported on this platform")
    }
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

/// Command for the suggestion overlay thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayCmd {
    /// Show (or update) the gray completion hint near the caret/pointer.
    Show { text: String },
    /// Hide the hint.
    Hide,
    /// Tear down and exit the overlay thread.
    Shutdown,
}

/// Draws the gray suggestion hint. Runs a blocking loop on its own thread; the
/// engine only ever sends non-blocking [`OverlayCmd`]s.
pub trait Overlay: Send {
    /// Blocking command loop; returns on `Shutdown` or fatal error.
    fn run(&mut self, rx: Receiver<OverlayCmd>) -> Result<()>;
    /// False on platforms without a real implementation — the engine then
    /// keeps autocomplete inert (no invisible suggestions, no key stealing).
    fn available(&self) -> bool {
        true
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
    /// Live control over autocomplete accept-key interception.
    pub intercept: InterceptHandle,
    /// Suggestion-hint overlay (may be a stub; check `available()`).
    pub overlay: Box<dyn Overlay>,
}

/// Command sent to a backend's interception machinery when its state changes
/// (Linux uses these to wake the XGrabKey thread).
#[derive(Clone, Debug)]
pub enum InterceptCmd {
    /// Start/stop swallowing the accept key (a suggestion became visible/hidden).
    Active(bool),
    /// The accept key was reassigned.
    Spec(HotkeySpec),
}

/// Shared with the capture backend: whether a suggestion is currently visible
/// (so the accept key must be swallowed) and which key accepts it.
#[derive(Clone)]
pub struct InterceptControl {
    active: Arc<AtomicBool>,
    spec: Arc<Mutex<HotkeySpec>>,
    cmds: Receiver<InterceptCmd>,
}

impl InterceptControl {
    /// True while a suggestion is visible and the accept key must be intercepted.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
    /// The current accept-key spec.
    pub fn current(&self) -> HotkeySpec {
        self.spec.lock().unwrap().clone()
    }
    /// Wake-up channel for backends that run a dedicated interception loop.
    pub fn commands(&self) -> &Receiver<InterceptCmd> {
        &self.cmds
    }
}

/// Engine-side handle: toggle interception and reassign the accept key.
pub struct InterceptHandle {
    active: Arc<AtomicBool>,
    spec: Arc<Mutex<HotkeySpec>>,
    cmds: Sender<InterceptCmd>,
}

impl InterceptHandle {
    /// Announce that a suggestion became visible (`true`) or was dismissed.
    pub fn set_active(&self, on: bool) {
        if self.active.swap(on, Ordering::SeqCst) != on {
            let _ = self.cmds.send(InterceptCmd::Active(on));
        }
    }
    /// Install a new accept-key spec.
    pub fn set_spec(&self, spec: HotkeySpec) {
        *self.spec.lock().unwrap() = spec.clone();
        let _ = self.cmds.send(InterceptCmd::Spec(spec));
    }
    /// The current accept-key spec.
    pub fn current(&self) -> HotkeySpec {
        self.spec.lock().unwrap().clone()
    }
}

/// No-op overlay for platforms without an implementation. Drains the channel
/// so senders never block; reports itself unavailable.
pub struct StubOverlay;

impl Overlay for StubOverlay {
    fn run(&mut self, rx: Receiver<OverlayCmd>) -> Result<()> {
        while let Ok(cmd) = rx.recv() {
            if matches!(cmd, OverlayCmd::Shutdown) {
                break;
            }
        }
        Ok(())
    }
    fn available(&self) -> bool {
        false
    }
}

/// Default accept-key spec (plain Tab) used to seed backends before the app
/// applies the configured key via [`InterceptHandle::set_spec`].
pub fn default_accept() -> HotkeySpec {
    HotkeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        key: "TAB".to_string(),
    }
}

/// Create a linked intercept control/handle pair seeded with `initial`.
pub fn intercept_channel(initial: HotkeySpec) -> (InterceptControl, InterceptHandle) {
    let active = Arc::new(AtomicBool::new(false));
    let spec = Arc::new(Mutex::new(initial));
    let (tx, rx) = crossbeam_channel::unbounded();
    (
        InterceptControl {
            active: active.clone(),
            spec: spec.clone(),
            cmds: rx,
        },
        InterceptHandle {
            active,
            spec,
            cmds: tx,
        },
    )
}

/// What an armed "press a key" capture will assign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTarget {
    /// The manual-conversion hotkey.
    ConvertHotkey,
    /// The autocomplete accept key.
    AcceptKey,
}

// AtomicU8 encoding of `Option<CaptureTarget>` for the shared capturing flag.
const CAPTURE_NONE: u8 = 0;
const CAPTURE_CONVERT: u8 = 1;
const CAPTURE_ACCEPT: u8 = 2;

/// Shared with the capture backend: the current hotkey, a "capture the next key"
/// flag, and a channel to report a newly captured key back to the app.
#[derive(Clone)]
pub struct HotkeyControl {
    spec: Arc<Mutex<HotkeySpec>>,
    capturing: Arc<AtomicU8>,
    updates: Sender<(CaptureTarget, HotkeySpec)>,
}

impl HotkeyControl {
    /// The currently active hotkey.
    pub fn current(&self) -> HotkeySpec {
        self.spec.lock().unwrap().clone()
    }
    /// True while waiting to capture the next keypress (for either target).
    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::SeqCst) != CAPTURE_NONE
    }
    /// Backend records a captured key: stop capturing, update the live convert
    /// spec when that was the target, and report it so the app can persist it.
    /// (The accept spec lives in `InterceptHandle`; the app applies it there.)
    pub fn record(&self, spec: HotkeySpec) {
        let target = match self.capturing.swap(CAPTURE_NONE, Ordering::SeqCst) {
            CAPTURE_CONVERT => CaptureTarget::ConvertHotkey,
            CAPTURE_ACCEPT => CaptureTarget::AcceptKey,
            _ => return,
        };
        if target == CaptureTarget::ConvertHotkey {
            *self.spec.lock().unwrap() = spec.clone();
        }
        let _ = self.updates.send((target, spec));
    }
}

/// App-side handle: start capture, receive the chosen key, and push a new
/// convert hotkey (e.g. on config reload).
pub struct HotkeyHandle {
    spec: Arc<Mutex<HotkeySpec>>,
    capturing: Arc<AtomicU8>,
    updates: Receiver<(CaptureTarget, HotkeySpec)>,
}

impl HotkeyHandle {
    /// Arm capture: the next keypress is assigned to `target`.
    pub fn begin_capture(&self, target: CaptureTarget) {
        let code = match target {
            CaptureTarget::ConvertHotkey => CAPTURE_CONVERT,
            CaptureTarget::AcceptKey => CAPTURE_ACCEPT,
        };
        self.capturing.store(code, Ordering::SeqCst);
    }
    /// Replace the live convert hotkey (the capture backend reads it per key),
    /// so a config reload can change it without restarting.
    pub fn set_spec(&self, spec: HotkeySpec) {
        *self.spec.lock().unwrap() = spec;
    }
    /// Channel of newly captured keys (for the engine's select loop).
    pub fn updates(&self) -> &Receiver<(CaptureTarget, HotkeySpec)> {
        &self.updates
    }
}

/// Create a linked control/handle pair seeded with `initial`.
pub fn hotkey_channel(initial: HotkeySpec) -> (HotkeyControl, HotkeyHandle) {
    let capturing = Arc::new(AtomicU8::new(CAPTURE_NONE));
    let spec = Arc::new(Mutex::new(initial));
    let (tx, rx) = crossbeam_channel::unbounded();
    (
        HotkeyControl {
            spec: spec.clone(),
            capturing: capturing.clone(),
            updates: tx,
        },
        HotkeyHandle {
            spec,
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
