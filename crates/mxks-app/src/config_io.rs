//! Loading, first-run creation, and reloading of the TOML config.

use anyhow::{Context, Result};
use mxks_core::config::{Config, DEFAULT_TEMPLATE};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Application config directory: `<config_dir>/mx-keyboard-switcher`.
pub fn app_config_dir() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("no config directory for this platform")?;
    Ok(dir.join("mx-keyboard-switcher"))
}

/// Path to the config file: `<config_dir>/mx-keyboard-switcher/config.toml`.
pub fn config_path() -> Result<PathBuf> {
    Ok(app_config_dir()?.join("config.toml"))
}

/// Platform-specific default directory for diagnostic logs.
pub fn default_log_dir() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    let dir = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("no state directory for this platform")?
        .join("mx-keyboard-switcher");

    #[cfg(target_os = "macos")]
    let dir = dirs::home_dir()
        .context("no home directory for this platform")?
        .join("Library")
        .join("Logs")
        .join("MX Keyboard Switcher");

    #[cfg(target_os = "windows")]
    let dir = dirs::data_local_dir()
        .context("no local data directory for this platform")?
        .join("MX Keyboard Switcher")
        .join("Logs");

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let dir = dirs::data_local_dir()
        .context("no local data directory for this platform")?
        .join("mx-keyboard-switcher")
        .join("logs");

    Ok(dir)
}

/// Effective log directory: the configured folder or the platform default.
pub fn log_dir(configured: &str) -> Result<PathBuf> {
    if configured.trim().is_empty() {
        default_log_dir()
    } else {
        Ok(PathBuf::from(configured))
    }
}

/// Path to the diagnostic log inside `directory`.
pub fn log_path(directory: &Path) -> PathBuf {
    directory.join("mxks.log")
}

/// Open the diagnostic log in append mode, rotating one oversized generation.
pub fn open_log_file(directory: &Path) -> Result<(PathBuf, File)> {
    let path = log_path(directory);
    let file = open_log_file_at(&path)?;
    Ok((path, file))
}

fn open_log_file_at(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating log directory {}", parent.display()))?;
    }

    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_LOG_BYTES => {
            let rotated = path.with_extension("log.1");
            match std::fs::remove_file(&rotated) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("removing old log {}", rotated.display()));
                }
            }
            std::fs::rename(path, &rotated).with_context(|| {
                format!(
                    "rotating diagnostic log {} to {}",
                    path.display(),
                    rotated.display()
                )
            })?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("reading log metadata {}", path.display()));
        }
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening diagnostic log {}", path.display()))
}

/// Load the config, creating a commented default file on first run. A malformed
/// file is reported and the built-in defaults are used instead of failing.
pub fn load() -> Config {
    match load_inner() {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("using defaults; failed to load config: {e:#}");
            Config::default()
        }
    }
}

/// Persist a new conversion hotkey into the config file.
pub fn save_hotkey(display: &str) -> Result<()> {
    save_key_line("hotkeys", "convert_last_word", display.into())
}

/// Persist a new autocomplete accept key into the config file.
pub fn save_accept_key(display: &str) -> Result<()> {
    save_key_line("autocomplete", "accept_key", display.into())
}

/// Persist the autocomplete on/off switch into the config file.
pub fn save_autocomplete_enabled(on: bool) -> Result<()> {
    save_key_line("autocomplete", "enabled", on.into())
}

/// Persist the "full auto inside terminals" switch into the config file.
pub fn save_terminal_auto(on: bool) -> Result<()> {
    save_key_line("terminals", "auto", on.into())
}

/// Persist the diagnostic file logging switch.
pub fn save_logging_enabled(on: bool) -> Result<()> {
    save_key_line("logging", "enabled", on.into())
}

/// Persist the selected diagnostic log directory.
pub fn save_log_directory(directory: &Path) -> Result<()> {
    save_key_line(
        "logging",
        "directory",
        directory.to_string_lossy().into_owned().into(),
    )
}

/// Update a TOML key without duplicating tables or discarding comments.
fn save_key_line(section: &str, key: &str, value: toml_edit::Value) -> Result<()> {
    let path = config_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DEFAULT_TEMPLATE.into(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let out = update_key(&text, section, key, value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn update_key(text: &str, section: &str, key: &str, mut value: toml_edit::Value) -> Result<String> {
    // Refuse to overwrite an invalid config with a partly repaired document.
    Config::from_toml(text).context("parsing config before saving")?;
    let mut document = text.parse::<toml_edit::Document>()?;
    if let Some(old) = document
        .get(section)
        .and_then(|table| table.get(key))
        .and_then(|item| item.as_value())
    {
        *value.decor_mut() = old.decor().clone();
    }
    document[section][key] = toml_edit::Item::Value(value);
    let out = document.to_string();
    Config::from_toml(&out).context("validating updated config")?;
    Ok(out)
}

/// Reload an existing file. A failed reload must leave the live config intact.
pub fn reload() -> Result<Config> {
    read_config(&config_path()?)
}

fn read_config(path: &Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Config::from_toml(&text).with_context(|| format!("parsing {}", path.display()))
}

fn load_inner() -> Result<Config> {
    load_at(&config_path()?)
}

fn load_at(path: &Path) -> Result<Config> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, DEFAULT_TEMPLATE)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        tracing::info!("created default config at {}", path.display());
        return Config::from_toml(DEFAULT_TEMPLATE).context("parsing default config");
    }
    read_config(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn partial_config_updates_preserve_tables_and_comments() {
        for input in [
            "# keep\n[autocomplete] # settings\nmin_prefix = 4\n",
            "# keep\nautocomplete = { min_prefix = 4 }\n",
            "# keep\nautocomplete.min_prefix = 4\n",
        ] {
            let output = update_key(input, "autocomplete", "enabled", false.into()).unwrap();
            let config = Config::from_toml(&output).unwrap();
            assert!(!config.autocomplete.enabled);
            assert_eq!(config.autocomplete.min_prefix, 4);
            assert!(output.contains("# keep"));
        }
    }

    #[test]
    fn config_updates_preserve_inline_comments_and_escape_values() {
        let text = "[logging]\nenabled = false # opt-in\n";
        let output = update_key(text, "logging", "enabled", true.into()).unwrap();
        assert!(output.contains("true # opt-in"));
        let directory = "C:\\Users\\a\\\"logs\"";
        let output = update_key(&output, "logging", "directory", directory.into()).unwrap();
        assert_eq!(
            Config::from_toml(&output).unwrap().logging.directory,
            directory
        );
        assert!(update_key("[logging", "logging", "enabled", true.into()).is_err());
    }

    #[test]
    fn first_run_matches_saved_config_and_reload_rejects_invalid_file() {
        let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mxks-config-{}-{serial}", std::process::id()));
        let path = dir.join("config.toml");
        let initial = load_at(&path).unwrap();
        let persisted = read_config(&path).unwrap();
        assert_eq!(initial.exclusions.apps, persisted.exclusions.apps);
        assert!(initial.exclusions.apps.iter().any(|app| app == "bitwarden"));
        std::fs::write(&path, "[invalid").unwrap();
        assert!(read_config(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[invalid");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn log_file_rotates_oversized_generation_then_appends() {
        let serial = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mxks-log-rotation-{}-{serial}", std::process::id()));
        let path = dir.join("mxks.log");
        let rotated = dir.join("mxks.log.1");
        std::fs::create_dir_all(&dir).unwrap();

        let old = File::create(&path).unwrap();
        old.set_len(MAX_LOG_BYTES + 1).unwrap();
        drop(old);

        let mut current = open_log_file_at(&path).unwrap();
        current.write_all(b"first").unwrap();
        current.flush().unwrap();
        drop(current);

        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            MAX_LOG_BYTES + 1
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"first");

        let mut reopened = open_log_file_at(&path).unwrap();
        reopened.write_all(b" second").unwrap();
        reopened.flush().unwrap();
        drop(reopened);

        assert_eq!(std::fs::read(&path).unwrap(), b"first second");
        assert_eq!(
            std::fs::metadata(&rotated).unwrap().len(),
            MAX_LOG_BYTES + 1
        );

        std::fs::remove_dir_all(dir).unwrap();
    }
}
