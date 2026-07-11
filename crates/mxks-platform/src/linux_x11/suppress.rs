//! Shared state that lets the capture thread drop our own injected key events.
//!
//! The injector taps real keycodes (letters, Backspace, Shift, Space). Before
//! sending each key-press it registers that keycode here; the capture thread
//! drops exactly that many presses of that keycode. This is order-independent
//! and robust to interleaving with a fast typist.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

/// Per-keycode count of injected key-presses still expected to echo back.
#[derive(Clone)]
pub struct Suppress {
    pending: Arc<Vec<AtomicU16>>,
}

impl Default for Suppress {
    fn default() -> Self {
        Suppress {
            pending: Arc::new((0..256).map(|_| AtomicU16::new(0)).collect()),
        }
    }
}

impl Suppress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register that we are about to inject one key-press of `keycode`.
    pub fn expect(&self, keycode: u8) {
        self.pending[keycode as usize].fetch_add(1, Ordering::SeqCst);
    }

    /// Called by the capture thread for each observed key-down. Returns true if
    /// this is one of our injected presses and should be dropped.
    pub fn should_drop(&self, keycode: u8) -> bool {
        let slot = &self.pending[keycode as usize];
        let mut cur = slot.load(Ordering::SeqCst);
        while cur > 0 {
            match slot.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
        false
    }
}
