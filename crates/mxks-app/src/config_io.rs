//! Loading, first-run creation, and reloading of the TOML config.

use anyhow::{Context, Result};
use mxks_core::config::{Config, DEFAULT_TEMPLATE};
use std::path::PathBuf;

/// Path to the config file: `<config_dir>/mx-keyboard-switcher/config.toml`.
pub fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("no config directory for this platform")?;
    Ok(dir.join("mx-keyboard-switcher").join("config.toml"))
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

/// Persist a new conversion hotkey into the config file, preserving comments by
/// replacing just the `convert_last_word` line.
pub fn save_hotkey(display: &str) -> Result<()> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_TEMPLATE.to_string());

    let mut out = String::new();
    let mut replaced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("convert_last_word") {
            out.push_str(&format!("convert_last_word = \"{display}\"\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("\n[hotkeys]\nconvert_last_word = \"{display}\"\n"));
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
