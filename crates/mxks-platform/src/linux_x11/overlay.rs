//! Gray suggestion hint as a small override-redirect X11 window.
//!
//! True "ghost text" inside another application's text field is impossible
//! from outside the app, so the hint is our own tiny window: dark background,
//! gray text, no decorations, never focused. Text is drawn with a core X
//! `iso10646-1` bitmap font (covers Cyrillic) via `image_text16` — dated but
//! dependency-free and plenty for a one-word hint.
//!
//! Positioning is best-effort "near the caret": the X input-focus window is
//! usually the toolkit's focused widget, so the hint goes just below its
//! origin; when that fails the hint follows the mouse pointer. True caret
//! geometry needs AT-SPI and is a follow-up.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Receiver;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, Char2b, ConnectionExt as _, CreateGCAux, CreateWindowAux, Font, Gcontext, PropMode,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_FROM_PARENT;

use crate::{Overlay, OverlayCmd};

/// Fonts tried in order; the first two are `misc-fixed` iso10646-1 (Cyrillic-
/// capable, shipped in xfonts-base), "fixed" is the always-present Latin core
/// font kept as a last resort.
const FONTS: &[&str] = &[
    "-misc-fixed-medium-r-normal--15-140-75-75-c-90-iso10646-1",
    "-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1",
    "fixed",
];

const PAD_X: i16 = 5;
const PAD_Y: i16 = 3;
/// Offset below/right of the anchor point.
const OFFSET: i16 = 16;
/// ~85% opaque (compositor-dependent; opaque without one).
const OPACITY: u32 = (0.85 * u32::MAX as f64) as u32;

const BG_RGB: (u16, u16, u16) = (0x2020, 0x2020, 0x2020);
const FG_RGB: (u16, u16, u16) = (0xb0b0, 0xb0b0, 0xb0b0);

pub struct X11Overlay {
    gui: Option<Gui>,
}

impl X11Overlay {
    /// Connect and build the (unmapped) hint window. On any failure the
    /// overlay reports itself unavailable instead of erroring the backend.
    pub fn new() -> Self {
        match Gui::new() {
            Ok(gui) => X11Overlay { gui: Some(gui) },
            Err(e) => {
                tracing::warn!("suggestion overlay unavailable: {e:#}");
                X11Overlay { gui: None }
            }
        }
    }
}

impl Overlay for X11Overlay {
    fn run(&mut self, rx: Receiver<OverlayCmd>) -> Result<()> {
        loop {
            match rx.recv() {
                Ok(OverlayCmd::Show { text }) => {
                    if let Some(gui) = &mut self.gui {
                        if let Err(e) = gui.show(&text) {
                            tracing::debug!("overlay show failed: {e:#}");
                        }
                    }
                }
                Ok(OverlayCmd::Hide) => {
                    if let Some(gui) = &mut self.gui {
                        let _ = gui.hide();
                    }
                }
                Ok(OverlayCmd::Shutdown) | Err(_) => break,
            }
        }
        if let Some(gui) = &mut self.gui {
            let _ = gui.hide();
        }
        Ok(())
    }

    fn available(&self) -> bool {
        self.gui.is_some()
    }
}

struct Gui {
    conn: RustConnection,
    root: Window,
    win: Window,
    gc: Gcontext,
    font: Font,
    ascent: i16,
    descent: i16,
    screen_w: u16,
    screen_h: u16,
    mapped: bool,
}

impl Gui {
    fn new() -> Result<Self> {
        let (conn, screen_num) = RustConnection::connect(None).context("overlay connection")?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let screen_w = screen.width_in_pixels;
        let screen_h = screen.height_in_pixels;
        let cmap = screen.default_colormap;

        // Open the first font that exists.
        let font = conn.generate_id()?;
        let mut opened = false;
        for name in FONTS {
            if conn
                .open_font(font, name.as_bytes())
                .ok()
                .and_then(|c| c.check().ok())
                .is_some()
            {
                tracing::debug!("overlay font: {name}");
                opened = true;
                break;
            }
        }
        if !opened {
            return Err(anyhow!("no usable core X font (install xfonts-base)"));
        }

        // Font metrics from an empty extents query.
        let ext = conn.query_text_extents(font, &[])?.reply()?;
        let (ascent, descent) = (ext.font_ascent, ext.font_descent);

        let bg = conn
            .alloc_color(cmap, BG_RGB.0, BG_RGB.1, BG_RGB.2)?
            .reply()?
            .pixel;
        let fg = conn
            .alloc_color(cmap, FG_RGB.0, FG_RGB.1, FG_RGB.2)?
            .reply()?
            .pixel;

        let win = conn.generate_id()?;
        conn.create_window(
            COPY_FROM_PARENT as u8,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0, // CopyFromParent visual
            &CreateWindowAux::new()
                .background_pixel(bg)
                .override_redirect(1)
                .save_under(1),
        )?
        .check()
        .context("create overlay window")?;

        // Semi-transparency via the compositor, if one is running.
        let opacity_atom = conn
            .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")?
            .reply()?
            .atom;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            opacity_atom,
            AtomEnum::CARDINAL,
            &[OPACITY],
        )?;

        let gc = conn.generate_id()?;
        conn.create_gc(
            gc,
            win,
            &CreateGCAux::new().foreground(fg).background(bg).font(font),
        )?
        .check()
        .context("create overlay GC")?;

        conn.flush()?;
        Ok(Gui {
            conn,
            root,
            win,
            gc,
            font,
            ascent,
            descent,
            screen_w,
            screen_h,
            mapped: false,
        })
    }

    /// Best-effort anchor: just below the focused widget's origin, else the
    /// mouse pointer.
    fn anchor(&self) -> (i16, i16) {
        if let Some(p) = self.focus_anchor() {
            return p;
        }
        if let Some(p) = self
            .conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
        {
            return (p.root_x + OFFSET, p.root_y + OFFSET);
        }
        (OFFSET, OFFSET)
    }

    fn focus_anchor(&self) -> Option<(i16, i16)> {
        let focus = self.conn.get_input_focus().ok()?.reply().ok()?.focus;
        // 0 = None, 1 = PointerRoot; neither is a real window.
        if focus <= 1 || focus == self.root {
            return None;
        }
        let geo = self.conn.get_geometry(focus).ok()?.reply().ok()?;
        let tr = self
            .conn
            .translate_coordinates(focus, self.root, 0, geo.height as i16)
            .ok()?
            .reply()
            .ok()?;
        if !tr.same_screen {
            return None;
        }
        Some((tr.dst_x + OFFSET, tr.dst_y + 4))
    }

    fn show(&mut self, text: &str) -> Result<()> {
        let chars: Vec<Char2b> = text
            .encode_utf16()
            .map(|u| Char2b {
                byte1: (u >> 8) as u8,
                byte2: (u & 0xff) as u8,
            })
            .collect();
        if chars.is_empty() {
            return self.hide();
        }

        let ext = self.conn.query_text_extents(self.font, &chars)?.reply()?;
        let w = (ext.overall_width.max(1) as i16 + PAD_X * 2) as u16;
        let h = ((self.ascent + self.descent) + PAD_Y * 2) as u16;

        let (ax, ay) = self.anchor();
        let x = ax.min(self.screen_w.saturating_sub(w) as i16).max(0);
        let y = ay.min(self.screen_h.saturating_sub(h) as i16).max(0);

        self.conn.configure_window(
            self.win,
            &x11rb::protocol::xproto::ConfigureWindowAux::new()
                .x(x as i32)
                .y(y as i32)
                .width(w as u32)
                .height(h as u32)
                .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
        )?;
        if !self.mapped {
            self.conn.map_window(self.win)?;
            self.mapped = true;
        }
        self.conn.clear_area(false, self.win, 0, 0, w, h)?;
        self.conn
            .image_text16(self.win, self.gc, PAD_X, PAD_Y + self.ascent, &chars)?;
        self.conn.flush()?;
        Ok(())
    }

    fn hide(&mut self) -> Result<()> {
        if self.mapped {
            self.conn.unmap_window(self.win)?;
            self.mapped = false;
            self.conn.flush()?;
        }
        Ok(())
    }
}
