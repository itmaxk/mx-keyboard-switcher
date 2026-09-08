# Manual test checklist

Automated tests cover the core engine and (on Linux) the live capture/inject/XKB
pipeline. These manual steps verify end-to-end behaviour in real apps on each OS.

## Common setup

1. Enable both **Russian** and **English** layouts in the OS.
2. Build and run: `MXKS_LOG=debug cargo run --release`. On Linux, do this only
   in the isolated X11 environment described below, never on the working
   desktop display.
3. Open a plain text field (text editor, browser address bar, chat box).

## Diagnostic log

Diagnostic file logging is off by default. Enable **Diagnostic file logging**
in the tray or set `[logging] enabled = true` in the config. The default paths
are:

- Windows: `%LOCALAPPDATA%\MX Keyboard Switcher\Logs\mxks.log`
- Linux: `${XDG_STATE_HOME:-~/.local/state}/mx-keyboard-switcher/mxks.log`
- macOS: `~/Library/Logs/MX Keyboard Switcher/mxks.log`

Use **Choose log folder…** to select another directory and **Open log folder**
to open the effective directory. The startup diagnostic event records the path.

The log records startup, accepted conversion hotkeys, layout states, buffer
lengths, toggle invalidations, and each conversion's source, intended
replacement, and injected text. It therefore contains words handled by MXKS;
treat it as sensitive and inspect it before sharing. The current file is
appended across starts and rotated to `mxks.log.1` after it exceeds 4 MiB.

## Core scenarios (run on every OS)

Autocomplete scenarios #11–12 apply to Linux/X11 and Windows; the macOS overlay
is not implemented yet.

| # | Steps | Expected |
|---|-------|----------|
| 1 | With **EN** active, type `ghbdtn` then Space | Becomes `привет `, layout switches to RU |
| 2 | With **RU** active, type `руддщ` then Space | Becomes `hello `, layout switches to EN |
| 3 | Type a valid word `hello` then Space | Left unchanged |
| 4 | Type a valid word `привет` (RU) then Space | Left unchanged |
| 5 | Type `ghbdtn`, then press the hotkey (Pause) before Space | Converts to `привет `, layout switches |
| 6 | Type a password-like `qwerty123` then Space | Left unchanged (has digits) |
| 7 | Tray → toggle **Autocorrection** off, repeat #1 | No correction happens |
| 8 | Tray → toggle **Enabled** off, repeat #1 and #5 | Nothing happens |
| 9 | Type fast: `ghbdtn ghbdtn ghbdtn ` | Each corrected; no doubled/dropped characters |
| 10 | Edit config `threshold`, Tray → **Reload config** | New value takes effect |
| 11 | Import `готово=7`, type `г`, then continue to `готи` | `готово` is suggested after `г`; it disappears or changes at `готи` because the full prefix must match |
| 12 | Type the shown `готово` completion manually, then press the accept key | Overlay shows `[Tab: confirm]`; confirmation does not change text and stores exactly one new accept |
| 13 | Type `ghb` in an editor, click into a browser text field, type `ghbdtn` then Space | Exactly `привет ` appears — no doubled or leftover characters; the editor's `ghb` is forgotten (the hotkey converts nothing) |
| 14 | Repeat #13 but switch windows with Alt+Tab instead of clicking | Same: exactly `привет `, first correction in the new window is clean |
| 15 | With **EN** active, type `how`, then press the conversion hotkey twice | First becomes exactly `рщц ` with RU active; second becomes exactly `how ` with EN active |

## Tray and icon (run on every OS)

Run these checks with the default `tray` feature enabled:

1. Open the menu and confirm all sixteen actions are present, with separators
   between the behavior switches, key assignment, config/counters, diagnostics,
   and start-at-login/quit groups:
   **Enabled**, **Autocorrection**, **Autocomplete**, **Auto in terminals**,
   **Change hotkey (now: …)**, **Change accept key (now: …)**,
   **Cancel key assignment**,
   **Open config file**, **Reload config**, **Export autocomplete counters**,
   **Import autocomplete counters**, **Diagnostic file logging**,
   **Open log folder**, **Choose log folder…**, **Start at login**, and **Quit**.
2. Toggle **Enabled**, **Autocorrection**, **Autocomplete**,
   **Auto in terminals**, **Diagnostic file logging**, and **Start at login**.
   On macOS, **Autocomplete** and **Change accept key** must be disabled because
   the overlay is unavailable. Each supported checkmark must follow the
   current engine state immediately and remain correct after reopening the menu.
3. Start either key-capture action. While capture is active, both assignment
   rows must read **Press a key…** and both must be disabled. **Cancel key
   assignment** must become enabled. Choose it and verify the old bindings
   remain active and typing works normally. Start capture again, press a key,
   and confirm both rows return to
   **Change hotkey (now: …)** / **Change accept key (now: …)** with the current
   configured values.
   **Cancel key assignment** must be disabled outside capture.
4. Use **Open config file**, change a value, then use **Reload config** and
   confirm the corresponding checkmark or dynamic label is refreshed.
   Change the conversion hotkey and verify it works immediately. Introduce a
   TOML syntax error, reload, and verify the previous settings remain active.
5. Choose **Export autocomplete counters** and confirm
   `autocomplete-usage-transfer.toml` appears beside `config.toml`.
6. Copy that file to a second isolated configuration, choose **Import
   autocomplete counters**, and confirm the learned suggestion returns; repeat
   import and verify the count does not increase.
7. Choose a new log folder, enable diagnostic logging, and verify `mxks.log`
   appears there. **Open log folder** must open that directory. Disable logging
   and confirm the file stops growing; restart and verify both settings persist.
8. Choose **Quit**. The icon must disappear and the process must exit cleanly
   after the keyboard engine stops.

## Linux (X11)

- Verify in: a terminal, a GTK app (gedit/text editor), Firefox/Chromium.
- Confirm no infinite loop / echo (scenario #9): our injected events must not be
  re-captured.
- In GTK editor, Firefox, and Chromium, repeat scenarios #1 and #15 twenty
  times in a disposable X11 VM or isolated Xephyr/Xvfb session. Expect exactly
  one trailing Space and no echo, doubling, or dropped characters.
- On a Wayland session, confirm the startup warning appears and native Wayland
  apps are not captured (XWayland apps may be).


### Isolated Linux tray smoke

- Never run the tray/keyboard daemon on the working desktop display (in
  particular, never on `DISPLAY=:10`). Use a disposable X11 VM, or a nested
  Xephyr display such as `DISPLAY=:99` with its own D-Bus session and an
  SNI-capable panel. Do not point the nested session at the host D-Bus.
- In that isolated session, run the common tray checks above. Confirm the panel
  renders the embedded lime keyboard icon instead of resolving the theme icon
  `input-keyboard`.
- If a nested environment cannot provide an SNI-capable panel, do not weaken
  the check or use the working desktop as a fallback; repeat it in a disposable
  X11 VM.
- Separately verify the no-host fallback in an isolated session without an SNI
  host: startup logs a warning, tray creation is skipped, and the keyboard
  engine continues headless. Leave it idle and measure CPU over an interval:
  it must stay near zero, not consume an entire core. Repeat with a build
  without the `tray` feature. A closed tray-command channel must neither spin
  the engine nor stop keyboard processing.
- For the repository's automated X11 regression, run only
  `scripts/run-x11-live-tests.sh`; never invoke an ignored `x11_live` test
  directly.

## Windows

### Programmatic focus regression (Windows and macOS)

Save the following as a disposable HTML file and open it in a browser. Reload
before each run. Type `ghbdtn` into the first field within five seconds; the
timer must move focus without a click or keypress. Then press Space and the
conversion hotkey. The second field must remain `KEEP `, with no deletion or
conversion of text from the first field. Repeat after a manual conversion to
verify its toggle cannot cross the focus change. On Windows also repeat with
an autocomplete hint visible: it must disappear while idle after focus changes.

```html
<input id="first" autofocus><input id="second" value="KEEP">
<script>
setTimeout(() => {
  const field = document.getElementById('second');
  field.focus();
  field.setSelectionRange(field.value.length, field.value.length);
}, 5000);
</script>
```

Repeat with a programmatically activated native application/window and with
two native text controls. Include Firefox/Chromium (Windows), Safari/TextEdit
(macOS). Verify same-window field changes as well as whole-app activation.
Apps that do not expose native focus metadata/events remain a compatibility
limit; the test must not be marked passed based on cross-compilation alone.

### Windows input

- Verify in: Notepad, a browser, an RDP/remote session note.
- Confirm antivirus does not block the low-level hook (whitelist if needed).
- Confirm injected events aren't re-captured (dwExtraInfo tag works).
- In Chrome and Edge, open
  `data:text/html,<textarea autofocus style="width:80vw;height:40vh"></textarea>`.
  Repeat scenario #15 twenty times both without pauses and while holding the
  hotkey through autorepeat. Every cycle must end as exact `how `: no `hhooww`,
  missing characters, or second Space. Repeat scenario #1, then scenario #15 in
  Notepad.


### Windows tray and executable

- Build the native MSVC release executable and launch it from Explorer or
  `Start-Process`. Confirm no console window appears.
- Confirm Explorer shows the unique lime keyboard icon for
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
- With default terminal settings, type `ghbdtn ` in Terminal, iTerm2 and Ghostty:
  automatic replacement must stay off and the manual conversion hotkey must work.
  Verify configured exclusions in a normal text field of a password manager.
  Existing explicit `[terminals].apps` lists must be updated to include the
  relevant bundle IDs, or removed to use defaults.
- Revoke Accessibility permission and confirm there is no stale-word injection;
  restore permission/restart as required by macOS and repeat the focus regression.
- In TextEdit and a browser text field, repeat scenarios #1 and #15 twenty
  times. Expect one trailing Space, stable `how ` after the double toggle, and
  the system layout matching the resulting text after each step.

### macOS menu-bar UX

- Launch the release binary and confirm it creates no ordinary window and no
  Dock icon.
- Confirm the keyboard icon remains recognizable in both light and dark
  menu-bar appearances; it must not depend on its lime RGB colors to be
  legible.
- Run the common tray checks above and confirm the menu states and dynamic
  labels match the Linux/Windows behavior. **Quit** must remove the menu-bar
  icon and terminate the process.

## Resource checks

- Idle RSS < 20 MB (`ps`, Task Manager, Activity Monitor).
- Cold start < 300 ms.
- Correction latency feels instant (< 50 ms).
