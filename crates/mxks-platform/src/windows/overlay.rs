//! Gray suggestion hint as a layered, click-through, always-on-top window.
//!
//! The window never activates (`WS_EX_NOACTIVATE`) and ignores the mouse
//! (`WS_EX_TRANSPARENT`), so typing focus is never disturbed. Text is painted
//! in `WM_PAINT` from a shared buffer; `Show` updates the buffer, positions
//! the window at the caret (via `GetGUIThreadInFO`) or the mouse pointer, and
//! invalidates. The thread owns a small `PeekMessage` pump interleaved with
//! the command channel — the overlay has no interactive messages to serve, so
//! a 20 ms poll is plenty.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateSolidBrush, DrawTextW, EndPaint, FillRect, GetDC,
    InvalidateRect, ReleaseDC, SetBkMode, SetTextColor, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetForegroundWindow, GetGUIThreadInfo, GetSystemMetrics, GetWindowThreadProcessId,
    PeekMessageW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos, ShowWindow,
    TranslateMessage, GUITHREADINFO, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, SM_CXSCREEN,
    SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::{Overlay, OverlayCmd};

const BG: u32 = 0x0020_2020; // COLORREF is 0x00BBGGRR; gray is symmetric
const FG: u32 = 0x00B0_B0B0;
const ALPHA: u8 = 217; // ~85%
const PAD_X: i32 = 5;
const PAD_Y: i32 = 3;
const OFFSET: i32 = 16;

/// Text currently shown, UTF-16, painted by `wnd_proc` on WM_PAINT.
static TEXT: Mutex<Vec<u16>> = Mutex::new(Vec::new());

pub struct WinOverlay;

impl Overlay for WinOverlay {
    fn run(&mut self, rx: Receiver<OverlayCmd>) -> Result<()> {
        unsafe { run_loop(rx) }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_PAINT {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let brush = CreateSolidBrush(COLORREF(BG));
        FillRect(hdc, &ps.rcPaint, brush);
        SetTextColor(hdc, COLORREF(FG));
        SetBkMode(hdc, TRANSPARENT);
        let mut text = TEXT.lock().unwrap().clone();
        if !text.is_empty() {
            let mut rc = RECT {
                left: PAD_X,
                top: PAD_Y,
                right: ps.rcPaint.right - PAD_X,
                bottom: ps.rcPaint.bottom - PAD_Y,
            };
            DrawTextW(hdc, &mut text, &mut rc, DT_SINGLELINE | DT_NOPREFIX);
        }
        let _ = EndPaint(hwnd, &ps);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Screen position of the text caret in the foreground window, if reported.
unsafe fn caret_pos() -> Option<POINT> {
    let fg = GetForegroundWindow();
    if fg.is_invalid() {
        return None;
    }
    let tid = GetWindowThreadProcessId(fg, None);
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    GetGUIThreadInfo(tid, &mut gui).ok()?;
    if gui.hwndCaret.is_invalid() {
        return None;
    }
    let mut pt = POINT {
        x: gui.rcCaret.left,
        y: gui.rcCaret.bottom,
    };
    if !ClientToScreen(gui.hwndCaret, &mut pt).as_bool() {
        return None;
    }
    Some(pt)
}

unsafe fn anchor() -> POINT {
    if let Some(p) = caret_pos() {
        return POINT {
            x: p.x + 4,
            y: p.y + 4,
        };
    }
    let mut p = POINT::default();
    let _ = GetCursorPos(&mut p);
    POINT {
        x: p.x + OFFSET,
        y: p.y + OFFSET,
    }
}

unsafe fn show(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        let _ = ShowWindow(hwnd, SW_HIDE);
        return;
    }

    // Measure.
    let hdc = GetDC(Some(hwnd));
    let mut rc = RECT::default();
    let mut measure = wide.clone();
    DrawTextW(
        hdc,
        &mut measure,
        &mut rc,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    ReleaseDC(Some(hwnd), hdc);
    let w = (rc.right - rc.left) + PAD_X * 2;
    let h = (rc.bottom - rc.top) + PAD_Y * 2;

    *TEXT.lock().unwrap() = wide;

    let a = anchor();
    let x = a.x.clamp(0, (GetSystemMetrics(SM_CXSCREEN) - w).max(0));
    let y = a.y.clamp(0, (GetSystemMetrics(SM_CYSCREEN) - h).max(0));

    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        x,
        y,
        w,
        h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    let _ = InvalidateRect(Some(hwnd), None, true);
}

unsafe fn run_loop(rx: Receiver<OverlayCmd>) -> Result<()> {
    let hinstance = GetModuleHandleW(None).context("module handle")?;
    let class = w!("mxks_overlay");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class,
        w!(""),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(hinstance.into()),
        None,
    )
    .context("create overlay window")?;
    SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA).context("layered alpha")?;

    loop {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(OverlayCmd::Show { text }) => show(hwnd, &text),
            Ok(OverlayCmd::Hide) => {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            Ok(OverlayCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = DestroyWindow(hwnd);
    Ok(())
}
