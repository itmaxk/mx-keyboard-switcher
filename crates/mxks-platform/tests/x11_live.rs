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
fn backend_builds_and_finds_spare_keycode() {
    // Just constructing the backend exercises spare-keycode discovery,
    // XKB group detection, and connection setup.
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

/// Our own injected input must be suppressed: the injector types through a
/// spare keycode and tagged backspaces, and the capture thread must drop them.
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
