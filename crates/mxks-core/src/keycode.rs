//! Physical keys relevant to RU/EN typing.
//!
//! We key the word buffer on *physical* keys, not characters, so that
//! conversion between layouts is independent of the OS-reported keysym.
//! Each backend maps its native scancode/keycode into a [`PhysKey`].

/// A physical key on the main alphanumeric block, named by its US-QWERTY glyph.
///
/// The 33 letter/adjacent keys here are exactly the ones that produce a letter
/// in the English (US-QWERTY) *or* Russian (ЙЦУКЕН) layout. Non-letter keys
/// (digits, space, punctuation outside this set) are word boundaries and are
/// not stored as `PhysKey`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum PhysKey {
    Q,
    W,
    E,
    R,
    T,
    Y,
    U,
    I,
    O,
    P,
    BracketL,
    BracketR,
    A,
    S,
    D,
    F,
    G,
    H,
    J,
    K,
    L,
    Semicolon,
    Quote,
    Z,
    X,
    C,
    V,
    B,
    N,
    M,
    Comma,
    Period,
    Slash,
    Backtick,
}

impl PhysKey {
    /// All physical keys, in table order. Used to build lookup maps.
    pub const ALL: [PhysKey; 34] = {
        use PhysKey::*;
        [
            Q, W, E, R, T, Y, U, I, O, P, BracketL, BracketR, A, S, D, F, G, H, J, K, L, Semicolon,
            Quote, Z, X, C, V, B, N, M, Comma, Period, Slash, Backtick,
        ]
    };
}
