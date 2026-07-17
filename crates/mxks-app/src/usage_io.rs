//! Persistent autocomplete acceptance counters and portable transfer files.

use anyhow::{bail, Context, Result};
use mxks_core::{dict, layout::Lang, usage::WordUsage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const USAGE_FILE_VERSION: u32 = 1;
const CANONICAL_FILE_NAME: &str = "autocomplete-usage.toml";
const TRANSFER_FILE_NAME: &str = "autocomplete-usage-transfer.toml";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageFile {
    version: u32,
    usage: WordUsage,
}

/// In-memory autocomplete usage with optional canonical and transfer paths.
pub struct UsageStore {
    usage: WordUsage,
    canonical_path: Option<PathBuf>,
    transfer_path: Option<PathBuf>,
}

impl UsageStore {
    /// Load the canonical state. Any path, read, parse, or validation failure
    /// starts with empty counters so a damaged file cannot stop the daemon.
    pub fn load() -> Self {
        let dir = match crate::config_io::app_config_dir() {
            Ok(dir) => dir,
            Err(error) => {
                tracing::error!("using empty autocomplete counters; failed to locate config directory: {error:#}");
                return Self::memory(WordUsage::default());
            }
        };
        let canonical_path = dir.join(CANONICAL_FILE_NAME);
        let transfer_path = dir.join(TRANSFER_FILE_NAME);
        let usage = if canonical_path.exists() {
            match read_usage(&canonical_path) {
                Ok(usage) => usage,
                Err(error) => {
                    tracing::error!(
                        "using empty autocomplete counters; failed to load {}: {error:#}",
                        canonical_path.display()
                    );
                    WordUsage::default()
                }
            }
        } else {
            WordUsage::default()
        };
        Self::from_optional_paths(usage, Some(canonical_path), Some(transfer_path))
    }

    /// Create an in-memory-only store.
    pub fn memory(usage: WordUsage) -> Self {
        Self::from_optional_paths(usage, None, None)
    }

    pub fn usage(&self) -> &WordUsage {
        &self.usage
    }

    /// Record one accepted word. Memory remains updated if persistence fails.
    pub fn record_accept(&mut self, word: &str, lang: Lang) -> Result<u32> {
        let count = self.usage.increment(word, lang);
        if let Some(path) = &self.canonical_path {
            if let Err(error) = write_usage(path, &self.usage) {
                tracing::warn!(
                    "autocomplete counter learned in memory but failed to save {}: {error:#}",
                    path.display()
                );
                return Err(error);
            }
        }
        Ok(count)
    }

    /// Overwrite the portable transfer file with the current snapshot.
    pub fn export(&self) -> Result<PathBuf> {
        let path = self
            .transfer_path
            .as_ref()
            .context("autocomplete counter export unavailable without a config directory")?;
        write_usage(path, &self.usage)?;
        Ok(path.clone())
    }

    /// Max-merge a fully validated transfer file, saving before replacing memory.
    pub fn import_max(&mut self) -> Result<usize> {
        let transfer_path = self
            .transfer_path
            .as_ref()
            .context("autocomplete counter import unavailable without a config directory")?;
        let canonical_path = self
            .canonical_path
            .as_ref()
            .context("autocomplete counter import unavailable without a config directory")?;

        let imported = read_usage(transfer_path)?;
        let mut merged = self.usage.clone();
        let changed = merged.merge_max(&imported);
        write_usage(canonical_path, &merged)?;
        self.usage = merged;
        Ok(changed)
    }

    fn from_optional_paths(
        usage: WordUsage,
        canonical_path: Option<PathBuf>,
        transfer_path: Option<PathBuf>,
    ) -> Self {
        Self {
            usage,
            canonical_path,
            transfer_path,
        }
    }

    #[cfg(test)]
    fn from_paths(usage: WordUsage, canonical_path: PathBuf, transfer_path: PathBuf) -> Self {
        Self::from_optional_paths(usage, Some(canonical_path), Some(transfer_path))
    }
}

fn read_usage(path: &Path) -> Result<WordUsage> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading autocomplete counters from {}", path.display()))?;
    let file: UsageFile = toml::from_str(&text)
        .with_context(|| format!("parsing autocomplete counters from {}", path.display()))?;
    validate_file(file)
        .with_context(|| format!("validating autocomplete counters from {}", path.display()))
}

fn validate_file(file: UsageFile) -> Result<WordUsage> {
    if file.version != USAGE_FILE_VERSION {
        bail!(
            "unsupported autocomplete counter version {}; expected {}",
            file.version,
            USAGE_FILE_VERSION
        );
    }
    for (lang, word, count) in file.usage.iter() {
        if count == 0 {
            bail!("zero autocomplete counter for {word:?}");
        }
        if word.to_lowercase() != word {
            bail!("autocomplete counter key is not lowercase: {word:?}");
        }
        if !dict::contains(word, lang) {
            bail!("autocomplete counter word is not in the built-in dictionary: {word:?}");
        }
    }
    Ok(file.usage)
}

fn write_usage(path: &Path, usage: &WordUsage) -> Result<()> {
    let text = toml::to_string_pretty(&UsageFile {
        version: USAGE_FILE_VERSION,
        usage: usage.clone(),
    })
    .context("serializing autocomplete counters")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "creating autocomplete counter directory {}",
                parent.display()
            )
        })?;
    }
    std::fs::write(path, text)
        .with_context(|| format!("writing autocomplete counters to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn increment_n(usage: &mut WordUsage, word: &str, lang: Lang, count: u32) {
        for _ in 0..count {
            usage.increment(word, lang);
        }
    }

    #[test]
    fn transfer_round_trip_max_merge_is_deterministic_and_invalid_import_is_atomic() {
        let dir = std::env::temp_dir().join(format!("mxks-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let canonical = dir.join(CANONICAL_FILE_NAME);
        let transfer = dir.join(TRANSFER_FILE_NAME);

        let mut local = WordUsage::default();
        increment_n(&mut local, "готово", Lang::Ru, 5);
        write_usage(&canonical, &local).unwrap();
        let mut store = UsageStore::from_paths(local, canonical.clone(), transfer.clone());
        assert_eq!(store.export().unwrap(), transfer);
        let exported_bytes = std::fs::read(&transfer).unwrap();
        assert_eq!(store.export().unwrap(), transfer);
        assert_eq!(std::fs::read(&transfer).unwrap(), exported_bytes);

        let mut incoming = WordUsage::default();
        increment_n(&mut incoming, "готово", Lang::Ru, 3);
        increment_n(&mut incoming, "готовить", Lang::Ru, 4);
        write_usage(&transfer, &incoming).unwrap();
        let transfer_bytes = std::fs::read(&transfer).unwrap();
        write_usage(&transfer, &incoming).unwrap();
        assert_eq!(std::fs::read(&transfer).unwrap(), transfer_bytes);

        assert_eq!(store.import_max().unwrap(), 1);
        assert_eq!(store.usage().count("готово", Lang::Ru), 5);
        assert_eq!(store.usage().count("готовить", Lang::Ru), 4);
        assert_eq!(store.import_max().unwrap(), 0);

        let memory_before = store.usage().clone();
        let canonical_before = std::fs::read(&canonical).unwrap();
        std::fs::write(
            &transfer,
            "version = 2\n\n[usage.en]\n\n[usage.ru]\n\"готово\" = 9\n",
        )
        .unwrap();
        assert!(store.import_max().is_err());
        assert_eq!(store.usage(), &memory_before);
        assert_eq!(std::fs::read(&canonical).unwrap(), canonical_before);

        std::fs::write(
            &transfer,
            "version = 1\n\n[usage.en]\ndefinitelynotabuiltinwordzzzz = 7\n\n[usage.ru]\n",
        )
        .unwrap();
        assert!(store.import_max().is_err());
        assert_eq!(store.usage(), &memory_before);
        assert_eq!(std::fs::read(&canonical).unwrap(), canonical_before);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
