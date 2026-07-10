//! MX Keyboard Switcher — a fast, lightweight cross-platform Punto Switcher analog.
//!
//! Wiring: a platform capture thread feeds key events into the engine over a
//! channel; the tray sends commands over another; the engine owns the buffer,
//! detector, and corrector.

mod config_io;
mod corrector;
mod engine;
mod tray;

use std::path::Path;

use anyhow::{Context, Result};
use mxks_core::hotkey;
use mxks_platform::KeyEvent;

use crate::corrector::Corrector;
use crate::engine::{Command, Engine, Status};

fn main() {
    init_logging();
    if let Err(e) = run() {
        tracing::error!("fatal: {e:#}");
        eprintln!("mx-keyboard-switcher: {e:#}");
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

    tray::start(cmd_tx, status_rx);

    let corrector = Corrector::new(backend.injector, backend.layout);
    let mut app = Engine::new(config, corrector, backend.focus).with_status_channel(status_tx);

    tracing::info!("MX Keyboard Switcher running");
    app.run(key_rx, cmd_rx);
    Ok(())
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
