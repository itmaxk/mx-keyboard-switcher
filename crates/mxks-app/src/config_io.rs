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
