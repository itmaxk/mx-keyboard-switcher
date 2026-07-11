//! Portable parsing of a hotkey string like `"Pause"` or `"Ctrl+Shift+K"`.
//!
//! The parsed [`HotkeySpec`] carries the modifier flags plus a canonical,
//! uppercase key name. Each platform backend maps that name to its native key
//! code, so this stays OS-independent.

/// A parsed hotkey: modifier state plus a canonical key name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    /// Canonical uppercase key name, e.g. `"PAUSE"`, `"F5"`, `"K"`.
    pub key: String,
}

impl Default for HotkeySpec {
    fn default() -> Self {
        HotkeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
            key: "PAUSE".to_string(),
        }
    }
}

impl HotkeySpec {
    /// Human-readable, re-parseable form, e.g. `"Ctrl+Shift+K"` or `"Pause"`.
    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.meta {
            parts.push("Super");
        }
        let mut s = parts.join("+");
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(&self.key);
        s
    }
}

/// Parse a hotkey string. Components are separated by `+`; the last non-modifier
/// component is the key. Returns `None` if there is no key component.
pub fn parse(s: &str) -> Option<HotkeySpec> {
    let mut spec = HotkeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        key: String::new(),
    };
    for part in s.split('+') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => spec.ctrl = true,
            "SHIFT" => spec.shift = true,
            "ALT" | "OPTION" => spec.alt = true,
            "META" | "SUPER" | "WIN" | "CMD" | "COMMAND" => spec.meta = true,
            other => spec.key = canonical(other),
        }
    }
    if spec.key.is_empty() {
        None
    } else {
        Some(spec)
    }
}

/// Normalize common aliases to a canonical key name.
fn canonical(name: &str) -> String {
    match name {
        "BREAK" | "PAUSEBREAK" | "PAUSE/BREAK" => "PAUSE",
        "SCROLL" | "SCRLK" => "SCROLLLOCK",
        "ESC" => "ESCAPE",
        "RETURN" => "ENTER",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_key() {
        let h = parse("Pause").unwrap();
        assert_eq!(h.key, "PAUSE");
        assert!(!h.ctrl && !h.shift && !h.alt && !h.meta);
    }

    #[test]
    fn chord() {
        let h = parse("Ctrl+Shift+K").unwrap();
        assert!(h.ctrl && h.shift);
        assert_eq!(h.key, "K");
    }

    #[test]
    fn aliases() {
        assert_eq!(parse("Break").unwrap().key, "PAUSE");
        assert_eq!(parse("win+space").unwrap().key, "SPACE");
        assert!(parse("win+space").unwrap().meta);
    }

    #[test]
    fn no_key_is_none() {
        assert!(parse("Ctrl+Shift").is_none());
        assert!(parse("").is_none());
    }
}
