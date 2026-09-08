//! Read only Accessibility metadata: foreground PID and focused element.
//! No text values are requested. Calls have a short IPC timeout so an
//! unresponsive application cannot stall keyboard processing indefinitely.

use core_foundation::base::{CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSRunningApplication;

use crate::{FocusInfo, FocusState};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout: f32) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
}

// Only retained AXUIElement references live here, never AppKit UI objects.
// AX references are remote object handles with no owning run-loop affinity.
// They move with the engine and are accessed exclusively via &mut MacFocus;
// no concurrent messaging or mutation of the retained CF objects occurs.
struct AxElement(CFType);
unsafe impl Send for AxElement {}

impl PartialEq for AxElement {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl AxElement {
    fn attribute(&self, name: &str) -> Option<Self> {
        let name = CFString::new(name);
        let mut value = std::ptr::null();
        let error = unsafe {
            AXUIElementCopyAttributeValue(
                self.0.as_CFTypeRef(),
                name.as_concrete_TypeRef(),
                &mut value,
            )
        };
        if value.is_null() {
            return None;
        }
        let value = unsafe { CFType::wrap_under_create_rule(value) };
        if error != 0 || value.type_of() != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        Some(Self(value))
    }
}

#[derive(Default)]
pub struct MacFocus {
    last: Option<(i32, AxElement)>,
    app: Option<String>,
}

impl MacFocus {
    fn sample() -> Option<(i32, AxElement, String)> {
        let raw = unsafe { AXUIElementCreateSystemWide() };
        if raw.is_null() {
            return None;
        }
        let system = AxElement(unsafe { CFType::wrap_under_create_rule(raw) });
        if unsafe { AXUIElementSetMessagingTimeout(system.0.as_CFTypeRef(), 0.02) } != 0 {
            return None;
        }
        let application = system.attribute("AXFocusedApplication")?;
        if unsafe { AXUIElementSetMessagingTimeout(application.0.as_CFTypeRef(), 0.02) } != 0 {
            return None;
        }
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(application.0.as_CFTypeRef(), &mut pid) } != 0 || pid <= 0 {
            return None;
        }
        let element = application.attribute("AXFocusedUIElement")?;
        if system.attribute("AXFocusedApplication")? != application {
            return None; // activation raced the metadata queries
        }
        // PID comes from Accessibility, not a cached workspace active-app
        // property. Only immutable names are read from NSRunningApplication.
        let name = autoreleasepool(|_| {
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
            let mut names = Vec::new();
            if let Some(bundle) = app.bundleIdentifier() {
                names.push(bundle.to_string());
            }
            if let Some(name) = app.localizedName() {
                names.push(name.to_string());
            }
            (!names.is_empty()).then(|| names.join(" ").to_lowercase())
        })?;
        Some((pid, element, name))
    }
}

impl FocusInfo for MacFocus {
    fn monitors_focus(&self) -> bool {
        true
    }

    fn poll_focus(&mut self) -> FocusState {
        let Some((pid, element, app)) = Self::sample() else {
            self.last = None;
            self.app = None;
            return FocusState::Unavailable;
        };
        let current = (pid, element);
        let changed = self.last.as_ref() != Some(&current);
        self.last = Some(current);
        self.app = Some(app);
        if changed {
            FocusState::Changed
        } else {
            FocusState::Unchanged
        }
    }

    fn focused_app(&self) -> Option<String> {
        self.app.clone()
    }
}
