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

/// Reproduce the phantom Control that real Pause keys emit (scancode carries a
/// Ctrl prefix): Control held while Pause is pressed must still fire the hotkey,
/// and the Control key must not surface as a buffer-resetting event.
#[test]
#[ignore = "requires a live X server"]
fn pause_with_phantom_control_fires() {
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    let mut saw_hotkey = false;
    let mut saw_reset = false;
    for _ in 0..30 {
        raw_key(37, true); // Control_L down (phantom)
        raw_tap(110); // Pause (state now includes Control)
        raw_key(37, false); // Control_L up
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
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300));

    let mut saw_hotkey = false;
    for _ in 0..30 {
        raw_tap(110); // Pause on this server
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

    // Arm capture and press Scroll_Lock (keycode 78 on this server).
    hotkey.begin_capture(mxks_platform::CaptureTarget::ConvertHotkey);
    let mut assigned = None;
    for _ in 0..30 {
        raw_tap(78); // Scroll_Lock
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
        raw_tap(78);
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
    // Constructing the backend exercises connection setup, XKB group
    // detection, and keymap loading.
    let _ = backend(HotkeySpec::default()).expect("backend builds");
}

/// XRecord capture must report a real (untagged) key press as a Letter.
#[test]
#[ignore = "requires a live X server"]
fn capture_reports_physical_key() {
    let Backend { mut capture, .. } = backend(HotkeySpec::default()).expect("backend");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let _ = capture.run(tx);
    });
    std::thread::sleep(Duration::from_millis(300)); // let RECORD start

    // Tap repeatedly and poll: the very first taps can race RECORD enabling, so
    // we retry for up to ~3s rather than relying on a single synthesized event.
    let mut saw_g = false;
    for _ in 0..30 {
        raw_tap(42); // evdev KEY_G (34) + 8
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
