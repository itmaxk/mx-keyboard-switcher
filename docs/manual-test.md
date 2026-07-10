# Manual test checklist

Automated tests cover the core engine and (on Linux) the live capture/inject/XKB
pipeline. These manual steps verify end-to-end behaviour in real apps on each OS.

## Common setup

1. Enable both **Russian** and **English** layouts in the OS.
2. Build and run: `MXKS_LOG=debug cargo run --release`.
3. Open a plain text field (text editor, browser address bar, chat box).

## Core scenarios (run on every OS)

| # | Steps | Expected |
|---|-------|----------|
| 1 | With **EN** active, type `ghbdtn` then Space | Becomes `привет `, layout switches to RU |
| 2 | With **RU** active, type `руддщ` then Space | Becomes `hello `, layout switches to EN |
| 3 | Type a valid word `hello` then Space | Left unchanged |
| 4 | Type a valid word `привет` (RU) then Space | Left unchanged |
| 5 | Type `ghbdtn`, then press the hotkey (Pause) before Space | Converts to `привет`, layout switches |
| 6 | Type a password-like `qwerty123` then Space | Left unchanged (has digits) |
| 7 | Tray → toggle **Autocorrection** off, repeat #1 | No correction happens |
| 8 | Tray → toggle **Enabled** off, repeat #1 and #5 | Nothing happens |
| 9 | Type fast: `ghbdtn ghbdtn ghbdtn ` | Each corrected; no doubled/dropped characters |
| 10 | Edit config `threshold`, Tray → **Reload config** | New value takes effect |

## Linux (X11)

- Verify in: a terminal, a GTK app (gedit/text editor), Firefox/Chromium.
- Confirm no infinite loop / echo (scenario #9): our injected events must not be
  re-captured.
- On a Wayland session, confirm the startup warning appears and native Wayland
  apps are not captured (XWayland apps may be).

## Windows

- Verify in: Notepad, a browser, an RDP/remote session note.
- Confirm antivirus does not block the low-level hook (whitelist if needed).
- Confirm injected events aren't re-captured (dwExtraInfo tag works).

## macOS

- First run: grant **Accessibility** (System Settings → Privacy & Security →
  Accessibility). Confirm the app shows an actionable error until granted.
- Set a real hotkey (Macs have no Pause): e.g. `convert_last_word = "F13"`.
- Confirm a focused **password field** is not corrected (Secure Input blinds the
  tap automatically).
- Verify in: TextEdit, a browser, Notes.

## Resource checks

- Idle RSS < 20 MB (`ps`, Task Manager, Activity Monitor).
- Cold start < 300 ms.
- Correction latency feels instant (< 50 ms).
