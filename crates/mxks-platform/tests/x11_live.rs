//! Live X11 integration tests. Ignored by default because they require a
//! running X server with `ru` and `us` layouts. Run explicitly with:
//!
//! ```sh
//! cargo test -p mxks-platform --test x11_live -- --ignored --test-threads=1
//! ```
#![cfg(target_os = "linux")]

use std::time::Duration;

use mxks_core::hotkey::HotkeySpec;
use mxks_core::keycode::PhysKey;
use mxks_core::layout::Lang;
use mxks_platform::{backend, Backend, KeyKind};

/// Synthesize a real (non-injected, untagged) key tap via XTEST on a fresh
/// connection, as if a physical key were pressed.
fn raw_tap(x_keycode: u8) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, _) = RustConnection::connect(None).unwrap();
    conn.xtest_fake_input(KEY_PRESS_EVENT, x_keycode, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    conn.xtest_fake_input(KEY_RELEASE_EVENT, x_keycode, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    conn.flush().unwrap();
}

/// Press or release a single keycode (no auto-release), to build sequences.
fn raw_key(x_keycode: u8, press: bool) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, _) = RustConnection::connect(None).unwrap();
    let ty = if press {
        KEY_PRESS_EVENT
    } else {
        KEY_RELEASE_EVENT
    };
    conn.xtest_fake_input(ty, x_keycode, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    conn.flush().unwrap();
}

/// Resolve the keycode currently mapped to `keysym`; live servers differ
/// (e.g. Pause is keycode 110 on the xrdp desktop but 127 on Xvfb).
fn keycode_of(keysym: u32) -> u8 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, _) = RustConnection::connect(None).unwrap();
    let min = conn.setup().min_keycode;
    let max = conn.setup().max_keycode;
    let reply = conn
        .get_keyboard_mapping(min, max - min + 1)
        .unwrap()
        .reply()
        .unwrap();
    for (i, syms) in reply
        .keysyms
        .chunks(reply.keysyms_per_keycode as usize)
        .enumerate()
    {
        if syms.contains(&keysym) {
            return min + i as u8;
        }
    }
    panic!("no keycode maps keysym {keysym:#x} on this server");
}

const XK_PAUSE: u32 = 0xff13;
const XK_SCROLL_LOCK: u32 = 0xff14;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_G: u32 = 0x0067;
const XK_RIGHT: u32 = 0xff53;

/// These tests inject raw XTEST events that the X server delivers to whatever
/// window currently has focus. On a shared desktop display that means phantom
/// Shift+Tab / Ctrl+C / letters landing in the user's terminals. Refuse to run
/// unless `MXKS_TEST_DISPLAY` (exported by `scripts/run-x11-live-tests.sh`)
/// names the isolated display we are connected to.
fn require_isolated_display() {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let allowed = std::env::var("MXKS_TEST_DISPLAY").unwrap_or_default();
    assert!(
        !allowed.is_empty() && display == allowed,
        "x11_live tests inject keystrokes into the focused window; run them only \
         via scripts/run-x11-live-tests.sh (DISPLAY={display:?}, MXKS_TEST_DISPLAY={allowed:?})"
    );
}

/// Reproduce the phantom Control that real Pause keys emit (scancode carries a
/// Ctrl prefix): Control held while Pause is pressed must still fire the hotkey,
/// and the Control key must not surface as a buffer-resetting event.
#[test]
#[ignore = "requires a live X server"]
fn pause_with_phantom_control_fires() {
    require_isolated_display();
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    let ctrl = keycode_of(XK_CONTROL_L);
    let pause = keycode_of(XK_PAUSE);
    let mut saw_hotkey = false;
    let mut saw_reset = false;
    for _ in 0..30 {
        raw_key(ctrl, true); // Control down (phantom)
        raw_tap(pause); // Pause (state now includes Control)
        raw_key(ctrl, false); // Control up
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            match ev.kind {
                KeyKind::Hotkey => saw_hotkey = true,
                KeyKind::Reset => saw_reset = true,
                _ => {}
            }
        }
        if saw_hotkey {
            break;
        }
    }
    assert!(
        saw_hotkey,
        "Pause with phantom Control did not fire the hotkey"
    );
    assert!(
        !saw_reset,
        "phantom Control produced a buffer-resetting event"
    );
}

/// The default Pause hotkey must be recognized regardless of its keycode
/// (110 here, 127 elsewhere) via keysym-name matching.
#[test]
#[ignore = "requires a live X server"]
fn pause_triggers_hotkey() {
    require_isolated_display();
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    let pause = keycode_of(XK_PAUSE);
    let mut saw_hotkey = false;
    for _ in 0..30 {
        raw_tap(pause);
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if ev.kind == KeyKind::Hotkey {
                saw_hotkey = true;
                break;
            }
        }
        if saw_hotkey {
            break;
        }
    }
    assert!(saw_hotkey, "Pause did not fire the conversion hotkey");
}

/// "Press a key to assign" captures the pressed key, reports it, and then that
/// key fires the hotkey.
#[test]
#[ignore = "requires a live X server"]
fn capture_reassigns_hotkey() {
    require_isolated_display();
    let Backend {
        mut capture,
        hotkey,
        ..
    } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    // Arm capture and press Scroll_Lock.
    let scroll_lock = keycode_of(XK_SCROLL_LOCK);
    hotkey.begin_capture(mxks_platform::CaptureTarget::ConvertHotkey);
    let mut assigned = None;
    for _ in 0..30 {
        raw_tap(scroll_lock);
        if let Ok((target, spec)) = hotkey.updates().recv_timeout(Duration::from_millis(100)) {
            assert_eq!(target, mxks_platform::CaptureTarget::ConvertHotkey);
            assigned = Some(spec);
            break;
        }
    }
    let spec = assigned.expect("capture did not report a hotkey");
    assert_eq!(spec.key, "SCROLLLOCK");

    // Now Scroll_Lock should fire the hotkey (no longer swallowed by capture).
    let mut saw_hotkey = false;
    for _ in 0..30 {
        raw_tap(scroll_lock);
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if ev.kind == KeyKind::Hotkey {
                saw_hotkey = true;
                break;
            }
        }
        if saw_hotkey {
            break;
        }
    }
    assert!(saw_hotkey, "reassigned Scroll_Lock hotkey did not fire");
}

#[test]
#[ignore = "requires a live X server with ru/us layouts"]
fn layout_switch_round_trips() {
    require_isolated_display();
    let mut b = backend(HotkeySpec::default()).expect("backend");

    b.layout.switch_to(Lang::En).expect("switch en");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(b.layout.current().expect("read"), Some(Lang::En));

    b.layout.switch_to(Lang::Ru).expect("switch ru");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(b.layout.current().expect("read"), Some(Lang::Ru));

    // Leave the system on English.
    b.layout.switch_to(Lang::En).ok();
}

#[test]
#[ignore = "requires a live X server"]
fn backend_builds() {
    require_isolated_display();
    // Constructing the backend exercises connection setup, XKB group
    // detection, and keymap loading.
    let _ = backend(HotkeySpec::default()).expect("backend builds");
}

/// XRecord capture must report a real (untagged) key press as a Letter.
#[test]
#[ignore = "requires a live X server"]
fn capture_reports_physical_key() {
    require_isolated_display();
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300)); // let RECORD start

    // Tap repeatedly and poll: the very first taps can race RECORD enabling, so
    // we retry for up to ~3s rather than relying on a single synthesized event.
    let g = keycode_of(XK_G);
    let mut saw_g = false;
    for _ in 0..30 {
        raw_tap(g);
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if let KeyKind::Letter {
                key: PhysKey::G, ..
            } = ev.kind
            {
                saw_g = true;
                break;
            }
        }
        if saw_g {
            break;
        }
    }
    assert!(
        saw_g,
        "capture did not report the synthesized 'G' key press"
    );
}

/// Our own injected input must be suppressed: the injector replays keycodes
/// registered for suppression, and the capture thread must drop their echoes.
#[test]
#[ignore = "requires a live X server"]
fn injected_input_is_suppressed() {
    require_isolated_display();
    let Backend {
        mut capture,
        mut injector,
        ..
    } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    injector.type_text("hi").expect("type");
    injector.backspaces(2).expect("backspaces");

    // None of the injected events should surface as engine events.
    std::thread::sleep(Duration::from_millis(200));
    let leaked: Vec<_> = rx.try_iter().collect();
    assert!(
        leaked.is_empty(),
        "injected events leaked to capture: {leaked:?}"
    );
}

/// Validate the accept-key **grab strategy** directly against the X server:
/// grabbing the accept key with its base modifier mask plus the CapsLock/NumLock
/// lock variants (never `AnyModifier`) must catch a bare accept-key press yet
/// leave modifier chords like Shift+Tab with the focused application — the exact
/// property that keeps Alt+Tab/Shift+Tab working while a suggestion is shown.
///
/// This exercises the grab semantics that `linux_x11::intercept` relies on,
/// deterministically (holding Shift and confirming the server sees it before
/// tapping Tab), without the RECORD/injection timing races of the full backend.
#[test]
#[ignore = "requires a live X server"]
fn accept_grab_masks_spare_shift_tab() {
    require_isolated_display();
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt as _, GrabMode, KeyButMask, ModMask, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
    };
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;

    const TAB: u8 = 23;
    const SHIFT_L: u8 = 50;
    // Base mask 0 (Tab has no modifiers) + CapsLock/NumLock variants, matching
    // `intercept::LOCK_VARIANTS`. Never AnyModifier.
    const LOCK_VARIANTS: [u16; 4] = [0, 0b10 /*Lock*/, 0b1_0000 /*Mod2*/, 0b1_0010];

    let (grab, screen) = RustConnection::connect(None).expect("grab conn");
    let root = grab.setup().roots[screen].root;
    for lock in LOCK_VARIANTS {
        grab.grab_key(
            false,
            root,
            ModMask::from(lock),
            TAB,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .expect("grab_key request")
        .check()
        .unwrap_or_else(|e| panic!("grab_key lock {lock:#b} failed: {e}"));
    }
    grab.flush().expect("flush grabs");
    std::thread::sleep(Duration::from_millis(150));

    let count_grabbed_presses = |timeout_ms: u64| -> usize {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut n = 0;
        while std::time::Instant::now() < deadline {
            while let Some(ev) = grab.poll_for_event().expect("poll") {
                if let Event::KeyPress(_) = ev {
                    n += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        n
    };
    // Discard anything queued from setup.
    let _ = count_grabbed_presses(50);

    let (inj, _) = RustConnection::connect(None).expect("inject conn");

    // Bare Tab must be delivered to the grabbing client (the swallow).
    inj.xtest_fake_input(KEY_PRESS_EVENT, TAB, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.xtest_fake_input(KEY_RELEASE_EVENT, TAB, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.flush().unwrap();
    assert_eq!(
        count_grabbed_presses(300),
        1,
        "bare Tab was not caught by the accept-key grab"
    );

    // Shift+Tab must NOT be caught: hold Shift, confirm the server reports it,
    // then tap Tab so the grab is evaluated against a real Shift modifier state.
    inj.xtest_fake_input(KEY_PRESS_EVENT, SHIFT_L, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.flush().unwrap();
    let mut shift_seen = false;
    for _ in 0..50 {
        if inj
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .mask
            .contains(KeyButMask::SHIFT)
        {
            shift_seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(shift_seen, "could not establish a held Shift state");
    inj.xtest_fake_input(KEY_PRESS_EVENT, TAB, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.xtest_fake_input(KEY_RELEASE_EVENT, TAB, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.flush().unwrap();
    let shifted = count_grabbed_presses(300);
    inj.xtest_fake_input(KEY_RELEASE_EVENT, SHIFT_L, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    inj.flush().unwrap();
    // Wait until the server confirms Shift cleared; otherwise the release can
    // race our disconnect and later tests see key events with Shift stuck on.
    for _ in 0..50 {
        if !inj
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .mask
            .contains(KeyButMask::SHIFT)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        shifted, 0,
        "Shift+Tab was wrongly caught by the accept-key grab (mask would steal the chord)"
    );

    // Ungrab so the server is left clean.
    for lock in LOCK_VARIANTS {
        let _ = grab.ungrab_key(TAB, root, ModMask::from(lock));
    }
    let _ = grab.flush();
}

/// End-to-end through the real backend: with the accept key set to the Right
/// arrow (the configured default that keeps Tab free for shell completion), the
/// interception thread must resolve the "RIGHT" name to a keycode, grab it while
/// active, and deliver an `Accept` — proving arrow keys are assignable accept
/// keys. (Injecting Right only moves the caret, so it is safe on the isolated
/// display.)
#[test]
#[ignore = "requires a live X server"]
fn intercept_resolves_and_grabs_right_arrow() {
    require_isolated_display();
    let Backend {
        mut capture,
        intercept,
        ..
    } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300)); // let RECORD + intercept start

    intercept.set_spec(HotkeySpec {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        key: "RIGHT".into(),
    });
    intercept.set_active(true);
    std::thread::sleep(Duration::from_millis(200)); // let the grab install

    let right = keycode_of(XK_RIGHT);
    let mut saw_accept = false;
    for _ in 0..20 {
        raw_tap(right);
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            if ev.kind == KeyKind::Accept {
                saw_accept = true;
                break;
            }
        }
        if saw_accept {
            break;
        }
    }
    intercept.set_active(false);
    assert!(
        saw_accept,
        "intercept did not resolve + grab the Right arrow accept key"
    );
}

/// A conversion triggered by a modifier hotkey (e.g. Ctrl+Pause) injects while
/// the user still holds the modifier. The injector must release held modifiers
/// first, or the injected keys become Ctrl+/Alt+ chords and corrupt the output.
#[test]
#[ignore = "requires a live X server"]
fn injection_releases_held_modifiers() {
    require_isolated_display();
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt as _, KeyButMask, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
    };
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let Backend { mut injector, .. } = backend(HotkeySpec::default()).expect("backend");
    // Hold Ctrl on a *persistent* connection: an ephemeral one (raw_key) drops
    // immediately and the server releases the key with it.
    let (probe, screen) = RustConnection::connect(None).expect("probe conn");
    let root = probe.setup().roots[screen].root;
    let ctrl = keycode_of(XK_CONTROL_L);

    // Physically hold Ctrl and confirm the server reports it.
    probe
        .xtest_fake_input(KEY_PRESS_EVENT, ctrl, 0, x11rb::NONE, 0, 0, 0)
        .unwrap();
    probe.flush().unwrap();
    let mut held = false;
    for _ in 0..50 {
        if probe
            .query_pointer(root)
            .unwrap()
            .reply()
            .unwrap()
            .mask
            .contains(KeyButMask::CONTROL)
        {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(held, "could not establish a held Ctrl state");

    // Any injection must clear the held modifier first.
    injector.type_text("a").expect("type");
    std::thread::sleep(Duration::from_millis(50));
    let still_held = probe
        .query_pointer(root)
        .unwrap()
        .reply()
        .unwrap()
        .mask
        .contains(KeyButMask::CONTROL);

    // cleanup regardless of outcome
    let _ = probe.xtest_fake_input(KEY_RELEASE_EVENT, ctrl, 0, x11rb::NONE, 0, 0, 0);
    let _ = probe.flush();
    assert!(
        !still_held,
        "injector left Ctrl held; injected keys would be Ctrl+ chords"
    );
}
