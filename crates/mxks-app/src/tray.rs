//! System tray. Linux uses `ksni` (StatusNotifierItem over DBus, no GTK). Other
//! platforms currently run headless (see the roadmap); the app is fully usable
//! via the config file and the hotkey without a tray.

use crossbeam_channel::{Receiver, Sender};

use crate::engine::{Command, Status};

/// Start the tray. On Linux (with the `tray` feature) this spawns background
/// DBus threads and a status listener; otherwise it is a no-op.
#[cfg(all(target_os = "linux", feature = "tray"))]
pub fn start(cmd_tx: Sender<Command>, status_rx: Receiver<Status>) {
    use ksni::blocking::TrayMethods;
    use ksni::menu::{CheckmarkItem, StandardItem};
    use ksni::{MenuItem, Tray};

    struct AppTray {
        cmd_tx: Sender<Command>,
        status: Status,
    }

    impl Tray for AppTray {
        fn id(&self) -> String {
            "mx-keyboard-switcher".into()
        }
        fn icon_name(&self) -> String {
            "input-keyboard".into()
        }
        fn title(&self) -> String {
            "MX Keyboard Switcher".into()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            let state = if self.status.enabled { "on" } else { "off" };
            ksni::ToolTip {
                title: "MX Keyboard Switcher".into(),
                description: format!("Switching {state}"),
                icon_name: "input-keyboard".into(),
                icon_pixmap: Vec::new(),
            }
        }
        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                CheckmarkItem {
                    label: "Enabled".into(),
                    checked: self.status.enabled,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::ToggleEnabled);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Autocorrection".into(),
                    checked: self.status.autocorrect,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::ToggleAutocorrect);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Autocomplete".into(),
                    checked: self.status.autocomplete,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::ToggleAutocomplete);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Auto in terminals".into(),
                    checked: self.status.terminal_auto,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::ToggleTerminalAuto);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: if self.status.capturing {
                        "Press a key…".into()
                    } else {
                        format!("Change hotkey (now: {})", self.status.hotkey)
                    },
                    enabled: !self.status.capturing,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::SetHotkey);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: if self.status.capturing {
                        "Press a key…".into()
                    } else {
                        format!("Change accept key (now: {})", self.status.accept_key)
                    },
                    enabled: !self.status.capturing,
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::SetAcceptKey);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Open config file".into(),
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::OpenConfig);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Reload config".into(),
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::ReloadConfig);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|t: &mut AppTray| {
                        let _ = t.cmd_tx.send(Command::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    let tray = AppTray {
        cmd_tx,
        status: Status {
            enabled: true,
            autocorrect: true,
            hotkey: "Pause".into(),
            capturing: false,
            autocomplete: true,
            terminal_auto: false,
            accept_key: "Tab".into(),
        },
    };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("tray unavailable ({e}); running without a tray icon");
            return;
        }
    };

    // Keep the tray's checkmarks in sync with engine state.
    std::thread::spawn(move || {
        while let Ok(status) = status_rx.recv() {
            handle.update(|t: &mut AppTray| t.status = status);
        }
    });
}

#[cfg(not(all(target_os = "linux", feature = "tray")))]
pub fn start(_cmd_tx: Sender<Command>, _status_rx: Receiver<Status>) {
    // Headless: tray disabled or not implemented for this platform.
    tracing::info!("tray not enabled; running headless");
}
