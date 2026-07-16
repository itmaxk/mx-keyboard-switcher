# Manual test checklist

Automated tests cover the core engine and (on Linux) the live capture/inject/XKB
pipeline. These manual steps verify end-to-end behaviour in real apps on each OS.

## Common setup

1. Enable both **Russian** and **English** layouts in the OS.
2. Build and run: `MXKS_LOG=debug cargo run --release`. On Linux, do this only
   in the isolated X11 environment described below, never on the working
   desktop display.
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

## Tray and icon (run on every OS)

Run these checks with the default `tray` feature enabled:

1. Confirm the tray/menu-bar shows the unique MX exchange icon, not a generic
   keyboard or missing-icon placeholder.
2. Open the menu and confirm all ten actions are present:
   **Enabled**, **Autocorrection**, **Autocomplete**, **Auto in terminals**,
   **Change hotkey (now: …)**, **Change accept key (now: …)**,
   **Open config file**, **Reload config**, **Start at login**, and **Quit**.
3. Toggle **Enabled**, **Autocorrection**, **Autocomplete**,
   **Auto in terminals**, and **Start at login**. Each checkmark must follow the
   current engine state immediately and remain correct after reopening the menu.
4. Start either key-capture action. While capture is active, both assignment
   rows must read **Press a key…** and both must be disabled; all other menu
   actions must remain available. Press a key and confirm both rows return to
   **Change hotkey (now: …)** / **Change accept key (now: …)** with the current
   configured values.
5. Use **Open config file**, change a value, then use **Reload config** and
   confirm the corresponding checkmark or dynamic label is refreshed.
6. Choose **Quit**. The icon must disappear and the process must exit cleanly
   after the keyboard engine stops.

## Linux (X11)

- Verify in: a terminal, a GTK app (gedit/text editor), Firefox/Chromium.
- Confirm no infinite loop / echo (scenario #9): our injected events must not be
  re-captured.
- On a Wayland session, confirm the startup warning appears and native Wayland
  apps are not captured (XWayland apps may be).


### Isolated Linux tray smoke

- Never run the tray/keyboard daemon on the working desktop display (in
  particular, never on `DISPLAY=:10`). Use a disposable X11 VM, or a nested
  Xephyr display such as `DISPLAY=:99` with its own D-Bus session and an
  SNI-capable panel. Do not point the nested session at the host D-Bus.
- In that isolated session, run the common tray checks above. Confirm the panel
  renders the embedded MX icon instead of resolving the theme icon
  `input-keyboard`.
- If a nested environment cannot provide an SNI-capable panel, do not weaken
  the check or use the working desktop as a fallback; repeat it in a disposable
  X11 VM.
- Separately verify the no-host fallback in an isolated session without an SNI
  host: startup logs a warning, tray creation is skipped, and the keyboard
  engine continues headless.
- For the repository's automated X11 regression, run only
  `scripts/run-x11-live-tests.sh`; never invoke an ignored `x11_live` test
  directly.

## Windows

- Verify in: Notepad, a browser, an RDP/remote session note.
- Confirm antivirus does not block the low-level hook (whitelist if needed).
- Confirm injected events aren't re-captured (dwExtraInfo tag works).


### Windows tray and executable

- Build the native MSVC release executable and launch it from Explorer or
  `Start-Process`. Confirm no console window appears.
- Confirm Explorer shows the unique MX icon for
  `mx-keyboard-switcher.exe`, and that the tray shows the same recognizable
  icon rather than a generic application icon.
- Run the common tray checks above, including checkmarks, both dynamic key
  labels, open/reload config, start-at-login, and clean Quit.
- Verify the native fatal path by temporarily adding these as the first
  executable lines of `main.rs::run`:
  `#[cfg(all(target_os = "windows", not(debug_assertions)))]` and
  `anyhow::bail!("tray smoke failure");`. Rebuild and launch the release
  executable. No console may appear. A native error dialog must show an
  error icon, title **MX Keyboard Switcher**, and exact body
  `MX Keyboard Switcher could not start:\n\ntray smoke failure`. Remove the
  temporary line immediately, rebuild the normal release, and confirm a search
  of the working tree finds no `tray smoke failure`.

## macOS

- First run: grant **Accessibility** (System Settings → Privacy & Security →
  Accessibility). Confirm the app shows an actionable error until granted.
- Set a real hotkey (Macs have no Pause): e.g. `convert_last_word = "F13"`.
- Confirm a focused **password field** is not corrected (Secure Input blinds the
  tap automatically).
- Verify in: TextEdit, a browser, Notes.

### macOS menu-bar UX

- Launch the release binary and confirm it creates no ordinary window and no
  Dock icon.
- Confirm the MX template icon remains recognizable in both light and dark
  menu-bar appearances; it must not depend on its indigo/lime RGB colors to be
  legible.
- Run the common tray checks above and confirm the menu states and dynamic
  labels match the Linux/Windows behavior. **Quit** must remove the menu-bar
  icon and terminate the process.

## Resource checks

- Idle RSS < 20 MB (`ps`, Task Manager, Activity Monitor).
- Cold start < 300 ms.
- Correction latency feels instant (< 50 ms).
