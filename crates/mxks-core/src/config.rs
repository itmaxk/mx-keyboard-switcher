//! TOML configuration schema, defaults, and the default file template.

use serde::{Deserialize, Serialize};

/// Full application configuration. Every field has a default so that a partial
/// or empty config file still loads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub hotkeys: Hotkeys,
    pub detection: Detection,
    pub exclusions: Exclusions,
    pub dictionary: Dictionary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Master switch for automatic wrong-layout correction.
    pub autocorrect: bool,
    /// Minimum physical-key length before a word is eligible for detection.
    pub min_word_len: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Key (or chord) that converts the last word and switches layout.
    /// Examples: "Pause", "Ctrl+Shift+K".
    pub convert_last_word: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Detection {
    /// Bigram score margin required to auto-correct. Higher = more conservative.
    pub threshold: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Exclusions {
    /// Process/app-name substrings where autocorrection is disabled.
    pub apps: Vec<String>,
    /// Typed forms that must never be auto-corrected.
    pub words: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictionary {
    /// Extra English words treated as valid (never corrected away from EN).
    pub extra_en: Vec<String>,
    /// Extra Russian words treated as valid.
    pub extra_ru: Vec<String>,
}

impl Default for General {
    fn default() -> Self {
        General {
            autocorrect: true,
            min_word_len: 3,
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            convert_last_word: "Pause".to_string(),
        }
    }
}

impl Default for Detection {
    fn default() -> Self {
        Detection { threshold: 3.0 }
    }
}

impl Config {
    /// Parse from TOML text. On error the caller should keep the previous config.
    pub fn from_toml(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }
}

/// Commented default config written on first run.
pub const DEFAULT_TEMPLATE: &str = r#"# MX Keyboard Switcher configuration

[general]
# Automatically fix words typed in the wrong layout (ghbdtn -> привет).
autocorrect = true
# Minimum word length (in keystrokes) before autocorrection considers a word.
min_word_len = 3

[hotkeys]
# Key that converts the last typed word and switches the system layout.
# Examples: "Pause", "Ctrl+Shift+K", "ScrollLock".
convert_last_word = "Pause"

[detection]
# Confidence margin required to auto-correct. Higher = fewer false positives.
threshold = 3.0

[exclusions]
# Substrings of application/process names where autocorrection is disabled.
apps = ["keepassxc", "1password", "bitwarden"]
# Typed forms that must never be auto-corrected.
words = []

[dictionary]
# Extra words treated as valid so they are never "corrected".
extra_en = []
extra_ru = []
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert!(c.general.autocorrect);
        assert_eq!(c.hotkeys.convert_last_word, "Pause");
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let c = Config::from_toml("").unwrap();
        assert!(c.general.autocorrect);
        assert_eq!(c.general.min_word_len, 3);
    }

    #[test]
    fn template_parses() {
        let c = Config::from_toml(DEFAULT_TEMPLATE).unwrap();
        assert_eq!(c.detection.threshold, 3.0);
        assert!(c.exclusions.apps.iter().any(|a| a == "keepassxc"));
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml("[general]\nautocorrect = false\n").unwrap();
        assert!(!c.general.autocorrect);
        // Untouched fields keep defaults.
        assert_eq!(c.general.min_word_len, 3);
    }
}
