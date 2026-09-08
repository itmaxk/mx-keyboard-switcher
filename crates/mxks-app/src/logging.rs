//! Runtime-switchable diagnostic file logging.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use mxks_core::config::Logging;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::{fmt, EnvFilter};

struct State {
    directory: PathBuf,
    file: Option<File>,
}

/// Shared control plane for the tracing file writer.
#[derive(Clone)]
pub struct LogController {
    state: Arc<Mutex<State>>,
}

impl LogController {
    fn new(directory: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                directory,
                file: None,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn enabled(&self) -> bool {
        self.lock().file.is_some()
    }

    pub fn directory(&self) -> PathBuf {
        self.lock().directory.clone()
    }

    pub fn path(&self) -> PathBuf {
        crate::config_io::log_path(&self.lock().directory)
    }

    /// Enable or disable file output. Enabling opens the file before changing
    /// the live writer, so a failure leaves the previous state untouched.
    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        if enabled {
            let directory = self.directory();
            let (_, file) = crate::config_io::open_log_file(&directory)?;
            self.lock().file = Some(file);
        } else {
            self.lock().file = None;
        }
        Ok(())
    }

    /// Change the destination directory. When logging is active, the new file
    /// is opened first and the writer switches over atomically.
    pub fn set_directory(&self, directory: &Path) -> Result<()> {
        let file = if self.enabled() {
            Some(crate::config_io::open_log_file(directory)?.1)
        } else {
            None
        };
        let mut state = self.lock();
        state.directory = directory.to_path_buf();
        if file.is_some() {
            state.file = file;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct FileWriter(LogController);

impl Write for FileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = self.0.lock();
        match &mut state.file {
            Some(file) => file.write(buffer),
            None => Ok(buffer.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.0.lock();
        match &mut state.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Install the process-wide tracing subscriber and apply the persisted setting.
/// Stderr remains available for fatal diagnostics; the menu controls file output.
pub fn init(config: &Logging) -> LogController {
    let directory = crate::config_io::log_dir(&config.directory).unwrap_or_else(|error| {
        let fallback = std::env::temp_dir().join("mx-keyboard-switcher");
        eprintln!(
            "mx-keyboard-switcher: cannot resolve the platform log directory ({error:#}); using {}",
            fallback.display()
        );
        fallback
    });
    let controller = LogController::new(directory);
    let file_writer = FileWriter(controller.clone());
    let filter = EnvFilter::try_from_env("MXKS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr.and(move || file_writer.clone()))
        .init();

    if config.enabled {
        if let Err(error) = controller.set_enabled(true) {
            tracing::warn!(
                log_path = %controller.path().display(),
                error = %format_args!("{error:#}"),
                "file logging unavailable; continuing with file logging disabled"
            );
        }
    }

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        file_logging = controller.enabled(),
        log_path = %controller.path().display(),
        "startup"
    );
    controller
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_controller_does_not_create_log_until_enabled() {
        let directory =
            std::env::temp_dir().join(format!("mxks-runtime-log-{}", std::process::id()));
        let path = crate::config_io::log_path(&directory);
        let _ = std::fs::remove_dir_all(&directory);

        let controller = LogController::new(directory.clone());
        assert!(!controller.enabled());
        assert!(!path.exists());
        let mut writer = FileWriter(controller.clone());
        writer.write_all(b"discarded").unwrap();
        writer.flush().unwrap();

        controller.set_enabled(true).unwrap();
        assert!(controller.enabled());
        assert!(path.exists());
        writer.write_all(b"recorded").unwrap();
        writer.flush().unwrap();
        controller.set_enabled(false).unwrap();
        assert!(!controller.enabled());
        writer.write_all(b"discarded again").unwrap();
        writer.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"recorded");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
