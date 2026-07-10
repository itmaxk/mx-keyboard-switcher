//! Layout read/switch via the Text Input Sources (TIS) API in Carbon.
//!
//! We identify layouts by their input-source ID string (e.g.
//! `com.apple.keylayout.US`, `com.apple.keylayout.Russian`).

use std::os::raw::c_void;

use anyhow::Result;
use core_foundation::array::CFArrayRef;
use core_foundation::base::{Boolean, CFIndex, CFRelease, TCFType};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::string::{CFString, CFStringRef};
use mxks_core::layout::Lang;

type TISInputSourceRef = *const c_void;
type OSStatus = i32;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
    fn TISCreateInputSourceList(
        properties: CFDictionaryRef,
        include_all_installed: Boolean,
    ) -> CFArrayRef;
    fn TISSelectInputSource(source: TISInputSourceRef) -> OSStatus;
    fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> *const c_void;
    static kTISPropertyInputSourceID: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: CFIndex) -> *const c_void;
}

pub struct MacLayout;

/// Read the input-source ID string of a TIS source.
unsafe fn source_id(source: TISInputSourceRef) -> Option<String> {
    if source.is_null() {
        return None;
    }
    let id_ref = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) as CFStringRef;
    if id_ref.is_null() {
        return None;
    }
    Some(
        CFString::wrap_under_get_rule(id_ref)
            .to_string()
            .to_lowercase(),
    )
}

fn lang_of_id(id: &str) -> Option<Lang> {
    if id.contains("russian") {
        Some(Lang::Ru)
    } else if id.contains(".us") || id.contains("abc") || id.contains("english") {
        Some(Lang::En)
    } else {
        None
    }
}

fn id_matches(lang: Lang, id: &str) -> bool {
    lang_of_id(id) == Some(lang)
}

impl crate::LayoutSwitcher for MacLayout {
    fn current(&self) -> Result<Option<Lang>> {
        unsafe {
            let src = TISCopyCurrentKeyboardInputSource();
            let lang = source_id(src).and_then(|id| lang_of_id(&id));
            if !src.is_null() {
                CFRelease(src as *const c_void);
            }
            Ok(lang)
        }
    }

    fn switch_to(&mut self, lang: Lang) -> Result<()> {
        unsafe {
            let list = TISCreateInputSourceList(std::ptr::null(), 0);
            if list.is_null() {
                return Ok(());
            }
            let count = CFArrayGetCount(list);
            for i in 0..count {
                let src = CFArrayGetValueAtIndex(list, i) as TISInputSourceRef;
                if let Some(id) = source_id(src) {
                    if id_matches(lang, &id) {
                        TISSelectInputSource(src);
                        break;
                    }
                }
            }
            CFRelease(list as *const c_void);
        }
        Ok(())
    }
}
