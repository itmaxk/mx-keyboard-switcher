#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! MX Keyboard Switcher — a fast, lightweight cross-platform Punto Switcher analog.
//!
//! Wiring: a platform capture thread feeds key events into the engine over a
//! channel; the tray sends commands over another; the engine owns the buffer,
//! detector, and corrector.

mod autostart;
mod config_io;
mod corrector;
mod engine;
mod tray;

use std::path::Path;

use anyhow::{Context, Result};
use mxks_core::hotkey;
use mxks_platform::{KeyEvent, OverlayCmd};

use crate::corrector::Corrector;
use crate::engine::{Command, Engine, Status};

fn main() {
    init_logging();
    if let Err(error) = run() {
        tracing::error!("fatal: {error:#}");
        report_fatal(&error);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config = config_io::load();

    let spec = hotkey::parse(&config.hotkeys.convert_last_word).unwrap_or_default();
    tracing::info!("convert hotkey: {:?}", spec);

    let mut backend = mxks_platform::backend(spec).context("initializing platform backend")?;

    // Channels: capture -> engine, tray -> engine, engine -> tray(status).
    let (key_tx, key_rx) = crossbeam_channel::unbounded::<KeyEvent>();
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (status_tx, status_rx) = crossbeam_channel::unbounded::<Status>();

    // Capture runs on its own thread and blocks there.
    let mut capture = std::mem::replace(&mut backend.capture, Box::new(NoopCapture));
    std::thread::spawn(move || {
        if let Err(e) = capture.run(key_tx) {
            tracing::error!("capture stopped: {e:#}");
        }
    });

    // Overlay runs on its own thread; the engine only sends non-blocking
    // commands. On platforms without an overlay (available() == false) the
    // engine never computes suggestions.
    let overlay_available = backend.overlay.available();
    let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded::<OverlayCmd>();
    let mut overlay = backend.overlay;
    std::thread::spawn(move || {
        if let Err(e) = overlay.run(overlay_rx) {
            tracing::error!("overlay stopped: {e:#}");
        }
    });

    // Seed the accept-key spec from config before it moves into the engine.
    match hotkey::parse(&config.autocomplete.accept_key) {
        Some(accept) => backend.intercept.set_spec(accept),
        None => tracing::warn!(
            "invalid autocomplete accept_key {:?}; keeping default Tab",
            config.autocomplete.accept_key
        ),
    }

    let corrector = Corrector::new(backend.injector, backend.layout);
    let mut app = Engine::new(config, corrector, backend.focus, backend.hotkey)
        .with_status_channel(status_tx)
        .with_autocomplete(overlay_tx, backend.intercept, overlay_available);
    let initial_status = app.status();

    tracing::info!("MX Keyboard Switcher running");

    #[cfg(all(any(target_os = "windows", target_os = "macos"), feature = "tray"))]
    {
        let (engine_done_tx, engine_done_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("mxks-engine".to_owned())
            .spawn(move || {
                app.run(key_rx, cmd_rx);
                let _ = engine_done_tx.send(());
            })
            .context("spawning engine thread")?;

        tray::run(cmd_tx, status_rx, initial_status, engine_done_rx)
    }

    #[cfg(not(all(any(target_os = "windows", target_os = "macos"), feature = "tray")))]
    {
        tray::start(cmd_tx, status_rx, initial_status);
        app.run(key_rx, cmd_rx);
        Ok(())
    }
}

#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn report_fatal(error: &anyhow::Error) {
    use windows::{
        core::{w, HSTRING},
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };

    let body = HSTRING::from(format!(
        "MX Keyboard Switcher could not start:\n\n{error:#}"
    ));
    unsafe {
        MessageBoxW(
            None,
            &body,
            w!("MX Keyboard Switcher"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(all(target_os = "windows", not(debug_assertions))))]
fn report_fatal(error: &anyhow::Error) {
    eprintln!("mx-keyboard-switcher: {error:#}");
}

/// Placeholder so we can move the real capture out of `Backend` into its thread.
struct NoopCapture;
impl mxks_platform::KeyCapture for NoopCapture {
    fn run(&mut self, _tx: crossbeam_channel::Sender<KeyEvent>) -> Result<()> {
        Ok(())
    }
}

fn init_logging() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("MXKS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

/// Open a file with the OS default handler.
pub fn open_path(path: &Path) {
    let p = path.as_os_str();
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(p).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(p)
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(p).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open {}: {e}", path.display());
    }
}
