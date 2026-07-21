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

/// Path to the persistent diagnostic log.
pub fn log_path() -> Result<PathBuf> {
    Ok(app_config_dir()?.join("mxks.log"))
}

/// Open the diagnostic log in append mode, rotating one oversized generation
/// at startup. The caller deliberately falls back to stderr if this fails.
pub fn open_log_file() -> Result<(PathBuf, File)> {
    let path = log_path()?;
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
    save_key_line("hotkeys", "convert_last_word", &format!("\"{display}\""))
}

/// Persist a new autocomplete accept key into the config file.
pub fn save_accept_key(display: &str) -> Result<()> {
    save_key_line("autocomplete", "accept_key", &format!("\"{display}\""))
}

/// Persist the autocomplete on/off switch into the config file.
pub fn save_autocomplete_enabled(on: bool) -> Result<()> {
    save_key_line("autocomplete", "enabled", if on { "true" } else { "false" })
}

/// Persist the "full auto inside terminals" switch into the config file.
pub fn save_terminal_auto(on: bool) -> Result<()> {
    save_key_line("terminals", "auto", if on { "true" } else { "false" })
}

/// Replace `key = <value>` inside `[section]` of the config file, preserving
/// comments and everything else line-by-line. Appends the section+key when the
/// file does not contain them yet.
fn save_key_line(section: &str, key: &str, value: &str) -> Result<()> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_TEMPLATE.to_string());

    let mut out = String::new();
    let mut replaced = false;
    let mut current_section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current_section = name.trim().to_string();
        } else if !replaced && current_section == section {
            // Match `key` followed by `=` or whitespace (not e.g. `key_other`).
            if let Some(rest) = t.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    out.push_str(&format!("{key} = {value}\n"));
                    replaced = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&format!("\n[{section}]\n{key} = {value}\n"));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn load_inner() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, DEFAULT_TEMPLATE)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        tracing::info!("created default config at {}", path.display());
        return Ok(Config::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Config::from_toml(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

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
