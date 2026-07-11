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
pub struct XkbLayout {
    conn: RustConnection,
    en_group: Option<u8>,
    ru_group: Option<u8>,
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
        let mut me = XkbLayout {
            conn,
            en_group: None,
            ru_group: None,
        };
        me.detect_groups()?;
        Ok(me)
    }

    /// Inspect the probe key's keysym in each group to classify EN vs RU.
    fn detect_groups(&mut self) -> Result<()> {
        let map = self.conn.get_keyboard_mapping(KC_PROBE, 1)?.reply()?;
        let per = map.keysyms_per_keycode as usize;
        let syms = &map.keysyms;
        // Groups occupy level pairs: group g -> base level g*2.
        let groups = per.div_ceil(2).min(4);
        for g in 0..groups {
            let idx = g * 2;
            let Some(sym) = syms.get(idx).copied() else {
                continue;
            };
            if self.en_group.is_none() && is_latin(sym) {
                self.en_group = Some(g as u8);
            }
            if self.ru_group.is_none() && is_cyrillic(sym) {
                self.ru_group = Some(g as u8);
            }
        }
        Ok(())
    }

    fn lang_of_group(&self, g: u8) -> Option<Lang> {
        if Some(g) == self.en_group {
            Some(Lang::En)
        } else if Some(g) == self.ru_group {
            Some(Lang::Ru)
        } else {
            None
        }
    }

    fn group_of_lang(&self, lang: Lang) -> Option<u8> {
        match lang {
            Lang::En => self.en_group,
            Lang::Ru => self.ru_group,
        }
    }

    pub fn current(&self) -> Result<Option<Lang>> {
        let state = self
            .conn
            .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
            .reply()?;
        let g: u8 = state.group.into();
        Ok(self.lang_of_group(g))
    }

    pub fn switch_to(&mut self, lang: Lang) -> Result<()> {
        let g = self
            .group_of_lang(lang)
            .ok_or_else(|| anyhow!("no XKB group for {:?}", lang))?;
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
