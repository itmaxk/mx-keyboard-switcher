//! Shared state that lets the capture thread drop our own injected key events.
//!
//! The injector taps real keycodes (letters, Backspace, Shift, Space). Before
//! sending each key-press it registers that keycode here; the capture thread
//! drops exactly that many presses of that keycode.
//!
//! Counting alone is not enough: a lost echo (e.g. swallowed while a grab or a
//! focus transition was in flight) would leave a stale counter that silently
//! eats the user's next real press of that keycode, and a real press racing a
//! correction could steal a count and let an injected echo leak into the
//! engine — both corrupt the very next correction. Two extra mechanisms close
//! those holes: an *injection window* (any unaccounted press observed while an
//! injection is in flight is dropped — it would interleave with the injected
//! backspaces and corrupt the text anyway) and *stale-counter expiry* (counters
//! left over well after the last injection ended are cleared, because every
//! genuine echo is already ordered into the RECORD stream before the injector
//! returns — see the sync round-trip in `inject.rs`).

use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Echoes still pending this long after the last injection ended are stale:
/// the injector's post-injection round-trip guarantees real echoes are already
/// in the RECORD stream when it returns, and the capture thread drains that
/// stream far faster than this.
const STALE_AFTER: Duration = Duration::from_millis(500);

/// What the capture thread should do with an observed key-press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// The echo of one of our injected presses — drop it.
    DropEcho,
    /// A press we did not inject, observed while an injection is in flight —
    /// drop it (it would interleave with the injected sequence).
    DropInjecting,
    /// A genuine user key-press.
    Real,
}

/// Per-keycode count of injected key-presses still expected to echo back,
/// plus the injection-window and staleness bookkeeping described above.
#[derive(Clone)]
pub struct Suppress {
    inner: Arc<Inner>,
}

struct Inner {
    pending: Vec<AtomicU16>,
    /// Nesting depth of injections currently in flight.
    injecting: AtomicUsize,
    /// Milliseconds since `base` when the last injection ended (0 = never).
    ended_at_ms: AtomicU64,
    base: Instant,
    /// [`STALE_AFTER`], overridable in tests.
    stale_after_ms: u64,
}

impl Default for Suppress {
    fn default() -> Self {
        Self::with_stale_after(STALE_AFTER)
    }
}

/// Marks an injection in flight; dropping it ends the window and records the
/// end time, so an early `?` return in the injector cannot leave it open.
pub struct InjectionGuard {
    inner: Arc<Inner>,
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        if self.inner.injecting.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner
                .ended_at_ms
                .store(self.inner.now_ms(), Ordering::SeqCst);
        }
    }
}

impl Inner {
    /// Monotonic milliseconds since construction, floored to 1 so a stored
    /// value of 0 can keep meaning "no injection has ended yet".
    fn now_ms(&self) -> u64 {
        (self.base.elapsed().as_millis() as u64).max(1)
    }
}

impl Suppress {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_stale_after(stale_after: Duration) -> Self {
        Suppress {
            inner: Arc::new(Inner {
                pending: (0..256).map(|_| AtomicU16::new(0)).collect(),
                injecting: AtomicUsize::new(0),
                ended_at_ms: AtomicU64::new(0),
                base: Instant::now(),
                stale_after_ms: stale_after.as_millis() as u64,
            }),
        }
    }

    /// Open an injection window; hold the guard for the whole injected batch.
    pub fn injection(&self) -> InjectionGuard {
        self.inner.injecting.fetch_add(1, Ordering::SeqCst);
        InjectionGuard {
            inner: self.inner.clone(),
        }
    }

    /// Register that we are about to inject one key-press of `keycode`.
    pub fn expect(&self, keycode: u8) {
        self.inner.pending[keycode as usize].fetch_add(1, Ordering::SeqCst);
    }

    /// Called by the capture thread for each observed key-down.
    pub fn filter(&self, keycode: u8) -> Filter {
        // Expire leftovers of a lost echo *before* consuming counters, so a
        // stale count cannot eat the user's first real press of that keycode.
        self.clear_stale();
        let slot = &self.inner.pending[keycode as usize];
        let mut cur = slot.load(Ordering::SeqCst);
        while cur > 0 {
            match slot.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Filter::DropEcho,
                Err(actual) => cur = actual,
            }
        }
        if self.inner.injecting.load(Ordering::SeqCst) > 0 {
            return Filter::DropInjecting;
        }
        Filter::Real
    }

    /// Zero all pending counters if they are leftovers of an injection that
    /// ended more than [`STALE_AFTER`] ago — an echo was lost (e.g. to a grab
    /// or a focus transition) and must not eat future real presses.
    fn clear_stale(&self) {
        if self.inner.injecting.load(Ordering::SeqCst) > 0 {
            return; // fresh expects of an in-flight injection are not stale
        }
        let ended = self.inner.ended_at_ms.load(Ordering::SeqCst);
        if ended == 0 || self.inner.now_ms().saturating_sub(ended) <= self.inner.stale_after_ms {
            return;
        }
        let mut cleared = 0u32;
        for slot in &self.inner.pending {
            cleared += u32::from(slot.swap(0, Ordering::SeqCst));
        }
        if cleared > 0 {
            tracing::warn!(
                "cleared {cleared} stale suppress counter(s): an injected key echo was lost"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_dropped_once_per_expect() {
        let s = Suppress::new();
        s.expect(38);
        s.expect(38);
        assert_eq!(s.filter(38), Filter::DropEcho);
        assert_eq!(s.filter(38), Filter::DropEcho);
        assert_eq!(s.filter(38), Filter::Real);
    }

    #[test]
    fn unexpected_press_during_injection_window_is_dropped() {
        let s = Suppress::new();
        let guard = s.injection();
        assert_eq!(s.filter(40), Filter::DropInjecting);
        drop(guard);
        assert_eq!(s.filter(40), Filter::Real);
    }

    #[test]
    fn nested_injection_windows_close_only_at_depth_zero() {
        let s = Suppress::new();
        let outer = s.injection();
        let inner = s.injection();
        drop(inner);
        assert_eq!(s.filter(40), Filter::DropInjecting);
        drop(outer);
        assert_eq!(s.filter(40), Filter::Real);
    }

    #[test]
    fn stale_counters_are_cleared_after_deadline() {
        // A lost echo (expect with no matching press) must stop eating real
        // presses once the injection is long over.
        let s = Suppress::with_stale_after(Duration::from_millis(5));
        {
            let _guard = s.injection();
            s.expect(38); // echo will be "lost"
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(s.filter(38), Filter::Real);
        assert_eq!(s.filter(38), Filter::Real);
    }

    #[test]
    fn counters_are_not_cleared_before_deadline() {
        let s = Suppress::new(); // real 500 ms deadline
        s.expect(38);
        drop(s.injection()); // injection just ended
        assert_eq!(s.filter(50), Filter::Real); // exercises the stale check
        assert_eq!(s.filter(38), Filter::DropEcho); // still pending, not cleared
    }

    #[test]
    fn counters_are_never_cleared_before_any_injection_ends() {
        let s = Suppress::with_stale_after(Duration::from_millis(1));
        s.expect(38);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(s.filter(50), Filter::Real);
        assert_eq!(s.filter(38), Filter::DropEcho);
    }
}
