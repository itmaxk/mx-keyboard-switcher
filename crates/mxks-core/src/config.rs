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
    pub terminals: Terminals,
    pub dictionary: Dictionary,
    pub autocomplete: Autocomplete,
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
    /// App-name substrings where the switcher is fully off — no automatic
    /// correction/suggestions AND no manual hotkey conversion (e.g. password
    /// managers).
    pub apps: Vec<String>,
    /// App-name substrings where automatic correction and suggestions are off
    /// but the manual conversion hotkey still works on demand. Terminals have
    /// their own `[terminals]` section; this is for any other manual-only apps.
    pub manual_only: Vec<String>,
    /// Typed forms that must never be auto-corrected.
    pub words: Vec<String>,
}

/// Terminals get their own tier: manual-only by default (shell commands must not
/// be auto-rewritten), with an opt-in to full auto and their own accept key so
/// the global `accept_key` can be Tab without stealing shell Tab-completion.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Terminals {
    /// App-name substrings (WM_CLASS, lowercased) treated as terminals.
    pub apps: Vec<String>,
    /// false = manual-only (auto off, hotkey works). true = full auto.
    pub auto: bool,
    /// Autocomplete accept key used inside terminals (when `auto` is true), kept
    /// separate from the global accept key so Tab stays free for the shell.
    pub accept_key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictionary {
    /// Extra English words treated as valid (never corrected away from EN).
    pub extra_en: Vec<String>,
    /// Extra Russian words treated as valid.
    pub extra_ru: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Autocomplete {
    /// Master switch for inline word suggestions.
    pub enabled: bool,
    /// Key that inserts the suggested completion. Examples: "Tab", "F8".
    pub accept_key: String,
    /// Minimum typed letters before a suggestion is shown.
    pub min_prefix: usize,
    /// Minimum letters the completion must add to be worth showing.
    pub min_remainder: usize,
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

impl Default for Autocomplete {
    fn default() -> Self {
        Autocomplete {
            enabled: true,
            accept_key: "Tab".to_string(),
            min_prefix: 3,
            min_remainder: 1,
        }
    }
}

impl Default for Terminals {
    fn default() -> Self {
        Terminals {
            apps: [
                "ptyxis",
                "gnome-terminal",
                "xterm",
                "konsole",
                "alacritty",
                "kitty",
                "terminator",
                "xfce4-terminal",
                "tilix",
                "wezterm",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            auto: false,
            accept_key: "Right".to_string(),
        }
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
# App-name substrings (WM_CLASS, lowercased) where the switcher is FULLY off —
# no automatic correction/suggestions and no manual hotkey (password managers).
apps = ["keepassxc", "1password", "bitwarden"]
# Extra manual-only apps (auto off, hotkey works). Terminals are configured in
# their own [terminals] section, so this is usually empty.
manual_only = []
# Typed forms that must never be auto-corrected.
words = []

[terminals]
# App-name substrings (WM_CLASS, lowercased) treated as terminals.
apps = ["ptyxis", "gnome-terminal", "konsole", "xterm", "alacritty", "kitty", "terminator", "xfce4-terminal", "tilix", "wezterm"]
# false (default): manual-only — auto-correct and suggestions are OFF (shell
#   commands are never rewritten), but the Pause hotkey still converts on demand.
# true: full auto, like any other app (uses accept_key below for suggestions).
auto = false
# Autocomplete accept key used INSIDE terminals when auto = true. Kept separate
# so the global accept_key can stay Tab without stealing shell Tab-completion.
accept_key = "Right"

[dictionary]
# Extra words treated as valid so they are never "corrected".
extra_en = []
extra_ru = []

[autocomplete]
# Suggest word completions while typing (shown in a small gray overlay).
enabled = true
# Key that inserts the suggested completion. Examples: "Tab", "F8".
accept_key = "Tab"
# Minimum typed letters before a suggestion is shown.
min_prefix = 3
# Minimum letters the completion must add to be worth showing.
min_remainder = 1
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
        assert!(c.terminals.apps.iter().any(|a| a == "ptyxis"));
        assert!(!c.terminals.auto);
        assert_eq!(c.terminals.accept_key, "Right");
        assert!(c.autocomplete.enabled);
        assert_eq!(c.autocomplete.accept_key, "Tab");
        assert_eq!(c.autocomplete.min_prefix, 3);
    }

    #[test]
    fn manual_only_defaults_empty_when_absent() {
        // A config without the new key still loads (empty list, not an error).
        let c = Config::from_toml("[exclusions]\napps = [\"keepassxc\"]\n").unwrap();
        assert!(c.exclusions.manual_only.is_empty());
    }

    #[test]
    fn terminals_section_parses_and_defaults() {
        // Terminals default to manual-only with their own accept key.
        let c = Config::default();
        assert!(c.terminals.apps.iter().any(|a| a == "ptyxis"));
        assert!(!c.terminals.auto);
        // A partial override keeps the rest of the defaults.
        let c = Config::from_toml("[terminals]\nauto = true\n").unwrap();
        assert!(c.terminals.auto);
        assert!(c.terminals.apps.iter().any(|a| a == "ptyxis"));
        assert_eq!(c.terminals.accept_key, "Right");
    }

    #[test]
    fn autocomplete_partial_override() {
        let c = Config::from_toml("[autocomplete]\nenabled = false\n").unwrap();
        assert!(!c.autocomplete.enabled);
        assert_eq!(c.autocomplete.accept_key, "Tab");
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml("[general]\nautocorrect = false\n").unwrap();
        assert!(!c.general.autocorrect);
        // Untouched fields keep defaults.
        assert_eq!(c.general.min_word_len, 3);
    }
}
