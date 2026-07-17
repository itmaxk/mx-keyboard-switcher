//! Cross-platform system tray.
//!
//! Linux uses `ksni` (StatusNotifierItem over DBus). Windows and macOS use
//! their native event loops through `tray-icon` and `winit`. Builds without the
//! `tray` feature keep the engine running headless.

use crossbeam_channel::{Receiver, Sender};

use crate::engine::{Command, Status};

#[cfg(feature = "tray")]
fn decode_tray_icon() -> anyhow::Result<(Vec<u8>, u32, u32)> {
    use anyhow::{bail, Context};
    use image::ImageFormat;

    let image = image::load_from_memory_with_format(
        include_bytes!("../assets/icon-tray.png"),
        ImageFormat::Png,
    )
    .context("failed to decode embedded tray icon")?
    .into_rgba8();
    let (width, height) = image.dimensions();
    if (width, height) != (32, 32) {
        bail!("embedded tray icon must be 32x32, got {width}x{height}");
    }
    Ok((image.into_raw(), width, height))
}

#[cfg(feature = "tray")]
struct MenuView {
    enabled: bool,
    autocorrect: bool,
    autocomplete: bool,
    terminal_auto: bool,
    autostart: bool,
    hotkey_text: String,
    accept_key_text: String,
    capture_actions_enabled: bool,
}

#[cfg(feature = "tray")]
impl From<&Status> for MenuView {
    fn from(status: &Status) -> Self {
        let capture_actions_enabled = !status.capturing;
        Self {
            enabled: status.enabled,
            autocorrect: status.autocorrect,
            autocomplete: status.autocomplete,
            terminal_auto: status.terminal_auto,
            autostart: status.autostart,
            hotkey_text: if capture_actions_enabled {
                format!("Change hotkey (now: {})", status.hotkey)
            } else {
                "Press a key…".into()
            },
            accept_key_text: if capture_actions_enabled {
                format!("Change accept key (now: {})", status.accept_key)
            } else {
                "Press a key…".into()
            },
            capture_actions_enabled,
        }
    }
}

/// Start the Linux tray and its status listener.
///
/// Failure to connect to DBus or an SNI host is intentionally non-fatal on
/// Linux: the keyboard engine remains fully usable without a tray.
#[cfg(all(target_os = "linux", feature = "tray"))]
pub fn start(cmd_tx: Sender<Command>, status_rx: Receiver<Status>, initial_status: Status) {
    use ksni::blocking::TrayMethods;
    use ksni::menu::{CheckmarkItem, StandardItem};
    use ksni::{Icon, MenuItem, Tray};

    struct AppTray {
        cmd_tx: Sender<Command>,
        status: Status,
        icon_pixmap: Vec<Icon>,
    }

    impl Tray for AppTray {
        fn id(&self) -> String {
            "mx-keyboard-switcher".into()
        }

        fn icon_name(&self) -> String {
            String::new()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon_pixmap.clone()
        }

        fn title(&self) -> String {
            "MX Keyboard Switcher".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let state = if self.status.enabled { "on" } else { "off" };
            ksni::ToolTip {
                title: "MX Keyboard Switcher".into(),
                description: format!("Switching {state}"),
                icon_name: String::new(),
                icon_pixmap: self.icon_pixmap.clone(),
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let view = MenuView::from(&self.status);
            vec![
                CheckmarkItem {
                    label: "Enabled".into(),
                    checked: view.enabled,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleEnabled);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Autocorrection".into(),
                    checked: view.autocorrect,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleAutocorrect);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Autocomplete".into(),
                    checked: view.autocomplete,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleAutocomplete);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Auto in terminals".into(),
                    checked: view.terminal_auto,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleTerminalAuto);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: view.hotkey_text,
                    enabled: view.capture_actions_enabled,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::SetHotkey);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: view.accept_key_text,
                    enabled: view.capture_actions_enabled,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::SetAcceptKey);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Open config file".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::OpenConfig);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Reload config".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ReloadConfig);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Export autocomplete counters".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ExportAutocompleteCounters);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Import autocomplete counters".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ImportAutocompleteCounters);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Start at login".into(),
                    checked: view.autostart,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleAutostart);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    let (mut rgba, width, height) = match decode_tray_icon() {
        Ok(icon) => icon,
        Err(error) => {
            tracing::warn!("tray unavailable ({error:#}); running without a tray icon");
            return;
        }
    };
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    let tray = AppTray {
        cmd_tx,
        status: initial_status,
        icon_pixmap: vec![Icon {
            width: width as i32,
            height: height as i32,
            data: rgba,
        }],
    };
    let handle = match tray.spawn() {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!("tray unavailable ({error}); running without a tray icon");
            return;
        }
    };

    std::thread::spawn(move || {
        while let Ok(status) = status_rx.recv() {
            handle.update(|tray: &mut AppTray| tray.status = status);
        }
    });
}

#[cfg(all(any(target_os = "windows", target_os = "macos"), feature = "tray"))]
mod native {
    use anyhow::{anyhow, Context};
    use crossbeam_channel::{never, Receiver, Sender};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder};
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ActiveEventLoop, EventLoop};

    use super::{decode_tray_icon, Command, MenuView, Status};

    const TOGGLE_ENABLED: &str = "toggle-enabled";
    const TOGGLE_AUTOCORRECT: &str = "toggle-autocorrect";
    const TOGGLE_AUTOCOMPLETE: &str = "toggle-autocomplete";
    const TOGGLE_TERMINAL_AUTO: &str = "toggle-terminal-auto";
    const SET_HOTKEY: &str = "set-hotkey";
    const SET_ACCEPT_KEY: &str = "set-accept-key";
    const OPEN_CONFIG: &str = "open-config";
    const RELOAD_CONFIG: &str = "reload-config";
    const EXPORT_AUTOCOMPLETE_COUNTERS: &str = "export-autocomplete-counters";
    const IMPORT_AUTOCOMPLETE_COUNTERS: &str = "import-autocomplete-counters";
    const TOGGLE_AUTOSTART: &str = "toggle-autostart";
    const QUIT: &str = "quit";

    pub(super) enum UserEvent {
        Menu(MenuEvent),
        Status(Status),
        EngineStopped,
    }

    struct MenuItems {
        enabled: CheckMenuItem,
        autocorrect: CheckMenuItem,
        autocomplete: CheckMenuItem,
        terminal_auto: CheckMenuItem,
        hotkey: MenuItem,
        accept_key: MenuItem,
        autostart: CheckMenuItem,
    }

    impl MenuItems {
        fn update(&self, view: &MenuView) {
            self.enabled.set_checked(view.enabled);
            self.autocorrect.set_checked(view.autocorrect);
            self.autocomplete.set_checked(view.autocomplete);
            self.terminal_auto.set_checked(view.terminal_auto);
            self.autostart.set_checked(view.autostart);
            self.hotkey.set_text(&view.hotkey_text);
            self.hotkey.set_enabled(view.capture_actions_enabled);
            self.accept_key.set_text(&view.accept_key_text);
            self.accept_key.set_enabled(view.capture_actions_enabled);
        }
    }

    struct UiState {
        _tray: TrayIcon,
        items: MenuItems,
    }

    pub(super) struct NativeApp {
        cmd_tx: Sender<Command>,
        status: Status,
        ui: Option<UiState>,
        pub(super) startup_error: Option<anyhow::Error>,
    }

    impl NativeApp {
        fn initialize(&mut self) -> anyhow::Result<UiState> {
            let view = MenuView::from(&self.status);
            let menu = Menu::new();
            let enabled =
                CheckMenuItem::with_id(TOGGLE_ENABLED, "Enabled", true, view.enabled, None);
            let autocorrect = CheckMenuItem::with_id(
                TOGGLE_AUTOCORRECT,
                "Autocorrection",
                true,
                view.autocorrect,
                None,
            );
            let autocomplete = CheckMenuItem::with_id(
                TOGGLE_AUTOCOMPLETE,
                "Autocomplete",
                true,
                view.autocomplete,
                None,
            );
            let terminal_auto = CheckMenuItem::with_id(
                TOGGLE_TERMINAL_AUTO,
                "Auto in terminals",
                true,
                view.terminal_auto,
                None,
            );
            let hotkey = MenuItem::with_id(
                SET_HOTKEY,
                &view.hotkey_text,
                view.capture_actions_enabled,
                None,
            );
            let accept_key = MenuItem::with_id(
                SET_ACCEPT_KEY,
                &view.accept_key_text,
                view.capture_actions_enabled,
                None,
            );
            let open_config = MenuItem::with_id(OPEN_CONFIG, "Open config file", true, None);
            let reload_config = MenuItem::with_id(RELOAD_CONFIG, "Reload config", true, None);
            let export_autocomplete_counters = MenuItem::with_id(
                EXPORT_AUTOCOMPLETE_COUNTERS,
                "Export autocomplete counters",
                true,
                None,
            );
            let import_autocomplete_counters = MenuItem::with_id(
                IMPORT_AUTOCOMPLETE_COUNTERS,
                "Import autocomplete counters",
                true,
                None,
            );
            let autostart = CheckMenuItem::with_id(
                TOGGLE_AUTOSTART,
                "Start at login",
                true,
                view.autostart,
                None,
            );
            let quit = MenuItem::with_id(QUIT, "Quit", true, None);

            menu.append_items(&[
                &enabled,
                &autocorrect,
                &autocomplete,
                &terminal_auto,
                &PredefinedMenuItem::separator(),
                &hotkey,
                &accept_key,
                &PredefinedMenuItem::separator(),
                &open_config,
                &reload_config,
                &export_autocomplete_counters,
                &import_autocomplete_counters,
                &autostart,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .context("failed to build tray menu")?;

            let (rgba, width, height) = decode_tray_icon()?;
            let icon = tray_icon::Icon::from_rgba(rgba, width, height)
                .map_err(|error| anyhow!("failed to create tray icon: {error}"))?;
            let builder = TrayIconBuilder::new()
                .with_id("mx-keyboard-switcher")
                .with_menu(Box::new(menu))
                .with_icon(icon)
                .with_tooltip("MX Keyboard Switcher")
                .with_menu_on_left_click(true);
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_as_template(true);
            let tray = builder.build().context("failed to create native tray")?;

            Ok(UiState {
                _tray: tray,
                items: MenuItems {
                    enabled,
                    autocorrect,
                    autocomplete,
                    terminal_auto,
                    hotkey,
                    accept_key,
                    autostart,
                },
            })
        }
    }

    impl ApplicationHandler<UserEvent> for NativeApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.ui.is_some() || self.startup_error.is_some() {
                return;
            }
            match self.initialize() {
                Ok(ui) => self.ui = Some(ui),
                Err(error) => {
                    self.startup_error = Some(error);
                    event_loop.exit();
                }
            }
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: winit::window::WindowId,
            _event: winit::event::WindowEvent,
        ) {
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
            match event {
                UserEvent::Menu(event) => {
                    let is_quit = event.id.0 == QUIT;
                    if let Some(command) = command_for_menu_id(&event.id) {
                        if self.cmd_tx.send(command).is_err() {
                            tracing::warn!("keyboard engine command channel disconnected");
                            if is_quit {
                                event_loop.exit();
                            }
                        }
                    }
                }
                UserEvent::Status(status) => {
                    self.status = status;
                    if let Some(ui) = &self.ui {
                        ui.items.update(&MenuView::from(&self.status));
                    }
                }
                UserEvent::EngineStopped => event_loop.exit(),
            }
        }
    }

    pub(super) fn command_for_menu_id(id: &MenuId) -> Option<Command> {
        match id.0.as_str() {
            TOGGLE_ENABLED => Some(Command::ToggleEnabled),
            TOGGLE_AUTOCORRECT => Some(Command::ToggleAutocorrect),
            TOGGLE_AUTOCOMPLETE => Some(Command::ToggleAutocomplete),
            TOGGLE_TERMINAL_AUTO => Some(Command::ToggleTerminalAuto),
            SET_HOTKEY => Some(Command::SetHotkey),
            SET_ACCEPT_KEY => Some(Command::SetAcceptKey),
            OPEN_CONFIG => Some(Command::OpenConfig),
            RELOAD_CONFIG => Some(Command::ReloadConfig),
            EXPORT_AUTOCOMPLETE_COUNTERS => Some(Command::ExportAutocompleteCounters),
            IMPORT_AUTOCOMPLETE_COUNTERS => Some(Command::ImportAutocompleteCounters),
            TOGGLE_AUTOSTART => Some(Command::ToggleAutostart),
            QUIT => Some(Command::Quit),
            _ => None,
        }
    }

    fn forward_status(
        status_rx: Receiver<Status>,
        engine_done: Receiver<()>,
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    ) {
        let mut statuses = status_rx;
        loop {
            crossbeam_channel::select! {
                recv(statuses) -> status => match status {
                    Ok(status) => {
                        if proxy.send_event(UserEvent::Status(status)).is_err() {
                            return;
                        }
                    }
                    Err(_) => statuses = never(),
                },
                recv(engine_done) -> _ => {
                    let _ = proxy.send_event(UserEvent::EngineStopped);
                    return;
                },
            }
        }
    }

    pub(super) fn run(
        cmd_tx: Sender<Command>,
        status_rx: Receiver<Status>,
        initial_status: Status,
        engine_done: Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut builder = EventLoop::<UserEvent>::with_user_event();
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
            builder.with_default_menu(false);
        }
        let event_loop = builder
            .build()
            .context("failed to create tray event loop")?;
        let menu_proxy = event_loop.create_proxy();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_proxy.send_event(UserEvent::Menu(event));
        }));
        let status_proxy = event_loop.create_proxy();
        std::thread::Builder::new()
            .name("mxks-tray-status".into())
            .spawn(move || forward_status(status_rx, engine_done, status_proxy))
            .context("failed to start tray status forwarder")?;

        let mut app = NativeApp {
            cmd_tx,
            status: initial_status,
            ui: None,
            startup_error: None,
        };
        let run_result = event_loop.run_app(&mut app);
        MenuEvent::set_event_handler::<fn(MenuEvent)>(None);
        run_result.context("native tray event loop failed")?;
        if let Some(error) = app.startup_error {
            return Err(error);
        }
        Ok(())
    }
}

/// Run the native tray event loop on the main thread.
#[cfg(all(any(target_os = "windows", target_os = "macos"), feature = "tray"))]
pub fn run(
    cmd_tx: Sender<Command>,
    status_rx: Receiver<Status>,
    initial_status: Status,
    engine_done: Receiver<()>,
) -> anyhow::Result<()> {
    native::run(cmd_tx, status_rx, initial_status, engine_done)
}

/// Start a headless build. The channels are deliberately accepted so callers
/// can use the same setup path regardless of whether the tray feature exists.
#[cfg(any(
    not(feature = "tray"),
    not(any(target_os = "linux", target_os = "windows", target_os = "macos"))
))]
pub fn start(_cmd_tx: Sender<Command>, _status_rx: Receiver<Status>, _initial_status: Status) {
    tracing::info!("tray not enabled; running headless");
}
#[cfg(all(test, feature = "tray"))]
mod tests {
    use super::{decode_tray_icon, MenuView};
    use crate::engine::Status;

    fn status(capturing: bool) -> Status {
        Status {
            enabled: false,
            autocorrect: true,
            hotkey: "Pause".into(),
            capturing,
            autocomplete: false,
            terminal_auto: true,
            accept_key: "Tab".into(),
            autostart: true,
        }
    }

    #[test]
    fn menu_view_reflects_status_and_capture() {
        let normal = MenuView::from(&status(false));
        assert!(!normal.enabled);
        assert!(normal.autocorrect);
        assert!(!normal.autocomplete);
        assert!(normal.terminal_auto);
        assert!(normal.autostart);
        assert_eq!(normal.hotkey_text, "Change hotkey (now: Pause)");
        assert_eq!(normal.accept_key_text, "Change accept key (now: Tab)");
        assert!(normal.capture_actions_enabled);

        let capturing = MenuView::from(&status(true));
        assert_eq!(capturing.hotkey_text, "Press a key…");
        assert_eq!(capturing.accept_key_text, "Press a key…");
        assert!(!capturing.capture_actions_enabled);
    }

    #[test]
    fn embedded_icon_is_32x32_rgba() {
        let (rgba, width, height) = decode_tray_icon().expect("embedded icon");
        assert_eq!((width, height), (32, 32));
        assert_eq!(rgba.len(), 32 * 32 * 4);
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}

#[cfg(all(
    test,
    any(target_os = "windows", target_os = "macos"),
    feature = "tray"
))]
mod native_tests {
    use super::native::command_for_menu_id;
    use crate::engine::Command;
    use tray_icon::menu::MenuId;

    #[test]
    fn menu_ids_map_to_commands() {
        let cases = [
            ("toggle-enabled", Command::ToggleEnabled),
            ("toggle-autocorrect", Command::ToggleAutocorrect),
            ("toggle-autocomplete", Command::ToggleAutocomplete),
            ("toggle-terminal-auto", Command::ToggleTerminalAuto),
            ("set-hotkey", Command::SetHotkey),
            ("set-accept-key", Command::SetAcceptKey),
            ("open-config", Command::OpenConfig),
            ("reload-config", Command::ReloadConfig),
            (
                "export-autocomplete-counters",
                Command::ExportAutocompleteCounters,
            ),
            (
                "import-autocomplete-counters",
                Command::ImportAutocompleteCounters,
            ),
            ("toggle-autostart", Command::ToggleAutostart),
            ("quit", Command::Quit),
        ];
        for (id, expected) in cases {
            assert_eq!(command_for_menu_id(&MenuId::new(id)), Some(expected));
        }
        assert_eq!(command_for_menu_id(&MenuId::new("unknown")), None);
    }
}
