//! Reading and switching the active XKB layout group on X11.
//!
//! We identify which group is English vs Russian by inspecting the keysym that
//! the probe key (`PhysKey::A`) produces in each group, so we don't depend on
//! group *names*.

use anyhow::{anyhow, Result};
use mxks_core::layout::Lang;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::keymap::KC_PROBE;

/// X11 layout reader/switcher.
///
/// The EN/RU group classification is re-read on every call rather than cached:
/// the keymap can be replaced while we run (xrdp reloads it on each reconnect,
/// `setxkbmap`, desktop keyboard settings), and stale group numbers would make
/// every switch land on the wrong layout until restart.
pub struct XkbLayout {
    conn: RustConnection,
}

/// Which XKB group holds each language in the current keymap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Groups {
    en: Option<u8>,
    ru: Option<u8>,
}

impl Groups {
    /// Classify groups by the probe key's base-level keysym in each group.
    fn from_mapping(keysyms_per_keycode: usize, syms: &[u32]) -> Self {
        let mut groups = Groups::default();
        // Groups occupy level pairs: group g -> base level g*2.
        for g in 0..keysyms_per_keycode.div_ceil(2).min(4) {
            let Some(&sym) = syms.get(g * 2) else {
                continue;
            };
            if groups.en.is_none() && is_latin(sym) {
                groups.en = Some(g as u8);
            }
            if groups.ru.is_none() && is_cyrillic(sym) {
                groups.ru = Some(g as u8);
            }
        }
        groups
    }

    fn lang_of(&self, g: u8) -> Option<Lang> {
        if Some(g) == self.en {
            Some(Lang::En)
        } else if Some(g) == self.ru {
            Some(Lang::Ru)
        } else {
            None
        }
    }

    fn group_of(&self, lang: Lang) -> Option<u8> {
        match lang {
            Lang::En => self.en,
            Lang::Ru => self.ru,
        }
    }
}

fn is_latin(sym: u32) -> bool {
    (0x61..=0x7a).contains(&sym) || (0x41..=0x5a).contains(&sym)
}

fn is_cyrillic(sym: u32) -> bool {
    // Legacy X Cyrillic keysyms, or Unicode-mapped Cyrillic keysyms.
    (0x6a0..=0x6ff).contains(&sym) || (0x0100_0400..=0x0100_04ff).contains(&sym)
}

impl XkbLayout {
    pub fn new() -> Result<Self> {
        let (conn, _) = RustConnection::connect(None)?;
        conn.xkb_use_extension(1, 0)?.reply()?;
        let me = XkbLayout { conn };
        let groups = me.groups()?;
        if groups.en.is_none() || groups.ru.is_none() {
            tracing::warn!(
                ?groups,
                "keymap lacks an EN or RU group; layout switching will fail until both are loaded"
            );
        }
        Ok(me)
    }

    /// Inspect the probe key's keysym in each group to classify EN vs RU.
    fn groups(&self) -> Result<Groups> {
        let map = self.conn.get_keyboard_mapping(KC_PROBE, 1)?.reply()?;
        Ok(Groups::from_mapping(
            map.keysyms_per_keycode as usize,
            &map.keysyms,
        ))
    }

    pub fn current(&self) -> Result<Option<Lang>> {
        // Pipeline both requests so the fresh classification costs no extra
        // round trip.
        let map = self.conn.get_keyboard_mapping(KC_PROBE, 1)?;
        let state = self.conn.xkb_get_state(xkb::ID::USE_CORE_KBD.into())?;
        let map = map.reply()?;
        let groups = Groups::from_mapping(map.keysyms_per_keycode as usize, &map.keysyms);
        let g: u8 = state.reply()?.group.into();
        Ok(groups.lang_of(g))
    }

    pub fn switch_to(&mut self, lang: Lang) -> Result<()> {
        let groups = self.groups()?;
        let g = groups.group_of(lang).ok_or_else(|| {
            anyhow!("no XKB group for {lang:?} in the current keymap ({groups:?})")
        })?;
        let group = xkb::Group::from(g);
        self.conn
            .xkb_latch_lock_state(
                xkb::ID::USE_CORE_KBD.into(),
                0u16.into(), // affect_mod_locks
                0u16.into(), // mod_locks
                true,        // lock_group
                group,       // group_lock
                0u16.into(), // affect_mod_latches
                false,       // latch_group
                0,           // group_latch
            )?
            // Round-trip so the group is actually applied before the caller
            // replays keycodes that depend on it.
            .check()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Probe key (A) keysyms laid out as XGetKeyboardMapping returns them:
    // [g0 base, g0 shift, g1 base, g1 shift, ...].
    const A_US_RU: [u32; 4] = [0x61, 0x41, 0x6c6, 0x6e6];
    const A_RU_US: [u32; 4] = [0x6c6, 0x6e6, 0x61, 0x41];

    #[test]
    fn classification_follows_keymap_order() {
        let us_ru = Groups::from_mapping(4, &A_US_RU);
        assert_eq!(
            us_ru,
            Groups {
                en: Some(0),
                ru: Some(1)
            }
        );
        let ru_us = Groups::from_mapping(4, &A_RU_US);
        assert_eq!(
            ru_us,
            Groups {
                en: Some(1),
                ru: Some(0)
            }
        );
        assert_eq!(ru_us.lang_of(0), Some(Lang::Ru));
        assert_eq!(ru_us.group_of(Lang::En), Some(1));
    }

    #[test]
    fn single_layout_keymap_has_no_second_group() {
        // What xrdp loads on reconnect: only the client's layout.
        let us = Groups::from_mapping(2, &A_US_RU[..2]);
        assert_eq!(
            us,
            Groups {
                en: Some(0),
                ru: None
            }
        );
        assert_eq!(us.group_of(Lang::Ru), None);
    }
}
