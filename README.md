<!-- Language: English | [Русский](README.ru.md) -->

# MX Keyboard Switcher

A fast, lightweight, cross-platform analog of Punto Switcher, written in Rust.

It fixes text typed in the wrong keyboard layout — e.g. you meant to type
`привет` but had the English layout active and got `ghbdtn`. MX Keyboard Switcher
detects this, erases the word, retypes it correctly, and switches the system
layout. It works fully **offline**, uses very little memory, and ships as a
single binary per OS.

> Languages in v1: **Russian ↔ English**. The architecture is layout-table +
> dictionary driven, so more pairs can be added later.

## Features

- **Automatic wrong-layout correction** (on by default): `ghbdtn ` → `привет `.
- **Manual conversion hotkey**: press **Pause/Break** to convert the current
  word and switch layout. The hotkey is configurable.
- **System tray**: enable/disable, toggle autocorrection, open/reload config.
- **TOML config** with per-app and per-word exclusions and a custom dictionary.
- **Privacy**: no network access at all; nothing leaves your machine.

## How it works

The word buffer is keyed on **physical keys**, not characters, so conversion is
layout-independent: the same key sequence is simply rendered through the other
layout's table. Detection combines a dictionary lookup, a character-bigram
language model, and an "impossible letter combination" heuristic, and is biased
hard toward leaving text untouched — a wrong correction is worse than a missed
one. The regression test suite enforces **zero false positives** on thousands of
valid words.

## Platform support

| OS | Capture | Inject | Layout switch | Notes |
|----|---------|--------|---------------|-------|
| **Linux (X11)** | XRecord | XTEST | XKB | Fully working & tested. Wayland: see below. |
| **Windows** | `WH_KEYBOARD_LL` | `SendInput` | `WM_INPUTLANGCHANGEREQUEST` | Implemented; test on-device. |
| **macOS** | `CGEventTap` | `CGEvent` | TIS | Implemented; needs Accessibility permission. |

### Linux / Wayland

v1 supports **X11** (and X11 apps under XWayland). Wayland does not expose a
portable global key-capture/injection API, so on a pure Wayland session the app
warns and runs degraded. A native Wayland backend (evdev/uinput + per-compositor
layout control) is planned.

### macOS

macOS requires the **Accessibility** permission (System Settings → Privacy &
Security → Accessibility) for the event tap to receive keystrokes. Password
fields are protected automatically by macOS Secure Input. Note that Mac keyboards
have no Pause/Break key — set a different `convert_last_word` hotkey (e.g.
`"F13"` or `"Ctrl+Shift+K"`) in the config.

### Windows

The low-level keyboard hook can look like a keylogger to antivirus software;
signing the binary and whitelisting help. No network access is a strong signal
that it is not exfiltrating anything.

## Install / Build

Requires a recent Rust toolchain (`rustup`, stable).

```sh
# Linux build dependency for the tray (StatusNotifierItem over DBus):
sudo apt-get install -y libdbus-1-dev pkg-config    # Debian/Ubuntu

# Build the release binary:
cargo build --release
# Binary: target/release/mx-keyboard-switcher

# Headless build without the tray (no libdbus needed):
cargo build --release --no-default-features
```

Run it:

```sh
./target/release/mx-keyboard-switcher
# Verbose logging:
MXKS_LOG=debug ./target/release/mx-keyboard-switcher
```

## Configuration

On first run a commented config file is created at:

- Linux: `~/.config/mx-keyboard-switcher/config.toml`
- macOS: `~/Library/Application Support/mx-keyboard-switcher/config.toml`
- Windows: `%APPDATA%\mx-keyboard-switcher\config.toml`

```toml
[general]
autocorrect = true        # automatic wrong-layout correction
min_word_len = 3          # ignore words shorter than this

[hotkeys]
convert_last_word = "Pause"   # or "Ctrl+Shift+K", "F13", ...

[detection]
threshold = 3.0           # higher = more conservative (fewer corrections)

[exclusions]
apps = ["keepassxc", "1password"]   # substrings of app names to skip
words = []                          # typed forms to never correct

[dictionary]
extra_en = []             # extra valid words (never "corrected")
extra_ru = []
```

Edit the file and choose **Reload config** from the tray (changing the hotkey
requires a restart).

## Limitations (v1)

- Autocorrection triggers on the **Space** separator; Enter/Tab/punctuation end
  the word but don't auto-correct (the manual hotkey still works).
- Per-app exclusions and password-field detection are best-effort and currently
  Linux-first.
- Wayland is X11/XWayland-only (see above).

## Development

```sh
cargo test --workspace            # unit + corpus tests (0 false positives gate)
cargo clippy --workspace --all-targets

# Live Linux/X11 integration tests (require an X server with ru/us layouts):
cargo test -p mxks-platform --test x11_live -- --ignored --test-threads=1
```

See [`docs/manual-test.md`](docs/manual-test.md) for the per-OS manual test
checklist.

## License

MIT — see [LICENSE](LICENSE).
