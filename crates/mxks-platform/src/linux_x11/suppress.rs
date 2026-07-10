//! Shared state that lets the capture thread drop our own injected events.
//!
//! Two mechanisms:
//! * Text is injected through a dedicated *spare* keycode that never occurs in
//!   normal typing, so any event on it is ours and is dropped unconditionally.
//! * Backspaces reuse the real Backspace keycode, so the injector bumps a
//!   counter before sending; the capture thread drops exactly that many
//!   Backspace key-downs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared suppression handshake between injector and capture.
#[derive(Clone)]
pub struct Suppress {
    inner: Arc<Inner>,
}

struct Inner {
    spare_keycode: u8,
    pending_backspaces: AtomicUsize,
}

impl Suppress {
    pub fn new(spare_keycode: u8) -> Self {
        Suppress {
            inner: Arc::new(Inner {
                spare_keycode,
                pending_backspaces: AtomicUsize::new(0),
            }),
        }
    }

    /// Called by the injector before sending `n` Backspace key events.
    pub fn expect_backspaces(&self, n: usize) {
        self.inner.pending_backspaces.fetch_add(n, Ordering::SeqCst);
    }

    /// Called by the capture thread for each observed key-down. Returns true if
    /// the event is one of ours and should be dropped.
    pub fn should_drop(&self, x_keycode: u8, is_backspace: bool) -> bool {
        if x_keycode == self.inner.spare_keycode {
            return true;
        }
        if is_backspace {
            // Decrement only if positive.
            let mut cur = self.inner.pending_backspaces.load(Ordering::SeqCst);
            while cur > 0 {
                match self.inner.pending_backspaces.compare_exchange_weak(
                    cur,
                    cur - 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => return true,
                    Err(actual) => cur = actual,
                }
            }
        }
        false
    }
}
