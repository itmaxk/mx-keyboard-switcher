//! Cross-platform system tray.
//!
//! Linux uses `ksni` (StatusNotifierItem over DBus). Windows and macOS use
//! their native event loops through `tray-icon` and `winit`. Builds without the
//! `tray` feature keep the engine running headless.

use crossbeam_channel::{Receiver, Sender};

#[cfg(feature = "tray")]
use mxks_core::layout::Lang;

use crate::engine::{Command, Status};

/// Embedded 32x32 tray icons for EN, RU and any other layout; the active
/// layout's key is drawn in front. The macOS menu bar renders template images
/// from alpha alone, so it gets variants with the letters cut out.
#[cfg(all(feature = "tray", not(target_os = "macos")))]
const TRAY_PNGS: [&[u8]; 3] = [
    include_bytes!("../assets/tray-en.png"),
    include_bytes!("../assets/tray-ru.png"),
    include_bytes!("../assets/tray-none.png"),
];
#[cfg(all(feature = "tray", target_os = "macos"))]
const TRAY_PNGS: [&[u8]; 3] = [
    include_bytes!("../assets/tray-en-template.png"),
    include_bytes!("../assets/tray-ru-template.png"),
    include_bytes!("../assets/tray-none-template.png"),
];

/// One tray icon per layout state, converted once for the tray backend.
#[cfg(feature = "tray")]
struct LayoutIcons<T> {
    en: T,
    ru: T,
    other: T,
}

#[cfg(feature = "tray")]
impl<T> LayoutIcons<T> {
    fn get(&self, layout: Option<Lang>) -> &T {
        match layout {
            Some(Lang::En) => &self.en,
            Some(Lang::Ru) => &self.ru,
            None => &self.other,
        }
    }
}

#[cfg(feature = "tray")]
fn decode_tray_icons<T>(
    mut convert: impl FnMut(Vec<u8>, u32, u32) -> anyhow::Result<T>,
) -> anyhow::Result<LayoutIcons<T>> {
    let [en, ru, other] = TRAY_PNGS;
    let mut load = |png: &[u8]| {
        let (rgba, width, height) = decode_tray_icon(png)?;
        convert(rgba, width, height)
    };
    Ok(LayoutIcons {
        en: load(en)?,
        ru: load(ru)?,
        other: load(other)?,
    })
}

#[cfg(feature = "tray")]
fn layout_text(layout: Option<Lang>) -> &'static str {
    match layout {
        Some(Lang::En) => "EN layout",
        Some(Lang::Ru) => "RU layout",
        None => "Other layout",
    }
}

#[cfg(feature = "tray")]
fn decode_tray_icon(png: &[u8]) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    use anyhow::{bail, Context};
    use image::ImageFormat;

    let image = image::load_from_memory_with_format(png, ImageFormat::Png)
        .context("failed to decode embedded tray icon")?
        .into_rgba8();
    let (width, height) = image.dimensions();
    if (width, height) != (32, 32) {
        bail!("embedded tray icon must be 32x32, got {width}x{height}");
    }
    Ok((image.into_raw(), width, height))
}

#[cfg(feature = "tray")]
fn choose_log_directory(current_log_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose diagnostic log folder");
    if let Some(directory) = current_log_path.parent() {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_folder()
}

#[cfg(feature = "tray")]
struct MenuView {
    enabled: bool,
    autocorrect: bool,
    autocomplete: bool,
    autocomplete_available: bool,
    terminal_auto: bool,
    autostart: bool,
    logging_enabled: bool,
    log_file_text: String,
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
            autocomplete_available: status.autocomplete_available,
            terminal_auto: status.terminal_auto,
            autostart: status.autostart,
            logging_enabled: status.logging_enabled,
            log_file_text: "Open log folder".into(),
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
        icons: LayoutIcons<Vec<Icon>>,
    }

    impl Tray for AppTray {
        fn id(&self) -> String {
            "mx-keyboard-switcher".into()
        }

        fn icon_name(&self) -> String {
            String::new()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icons.get(self.status.layout).clone()
        }

        fn title(&self) -> String {
            "MX Keyboard Switcher".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let state = if self.status.enabled { "on" } else { "off" };
            ksni::ToolTip {
                title: "MX Keyboard Switcher".into(),
                description: format!("{} · switching {state}", layout_text(self.status.layout)),
                icon_name: String::new(),
                icon_pixmap: self.icon_pixmap(),
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
                    enabled: view.autocomplete_available,
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
                    enabled: view.capture_actions_enabled && view.autocomplete_available,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::SetAcceptKey);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Cancel key assignment".into(),
                    enabled: !view.capture_actions_enabled,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::CancelKeyAssignment);
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
                MenuItem::Separator,
                CheckmarkItem {
                    label: "Diagnostic file logging".into(),
                    checked: view.logging_enabled,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::ToggleLogging);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: view.log_file_text,
                    activate: Box::new(|tray: &mut AppTray| {
                        let _ = tray.cmd_tx.send(Command::OpenLogDirectory);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Choose log folder…".into(),
                    activate: Box::new(|tray: &mut AppTray| {
                        if let Some(directory) = choose_log_directory(&tray.status.log_path) {
                            let _ = tray.cmd_tx.send(Command::SetLogDirectory(directory));
                        }
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
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

    let icons = match decode_tray_icons(|mut rgba, width, height| {
        // SNI pixmaps are ARGB32 in network byte order.
        let (pixels, remainder) = rgba.as_chunks_mut::<4>();
        debug_assert!(remainder.is_empty());
        for pixel in pixels {
            pixel.rotate_right(1);
        }
        Ok(vec![Icon {
            width: width as i32,
            height: height as i32,
            data: rgba,
        }])
    }) {
        Ok(icons) => icons,
        Err(error) => {
            tracing::warn!("tray unavailable ({error:#}); running without a tray icon");
            return;
        }
    };
    // ksni compares the pixmap after each update and signals NewIcon when the
    // layout changed it.
    let tray = AppTray {
        cmd_tx,
        status: initial_status,
        icons,
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

    use super::{decode_tray_icons, layout_text, Command, Lang, LayoutIcons, MenuView, Status};

    const TOGGLE_ENABLED: &str = "toggle-enabled";
    const TOGGLE_AUTOCORRECT: &str = "toggle-autocorrect";
    const TOGGLE_AUTOCOMPLETE: &str = "toggle-autocomplete";
    const TOGGLE_TERMINAL_AUTO: &str = "toggle-terminal-auto";
    const SET_HOTKEY: &str = "set-hotkey";
    const SET_ACCEPT_KEY: &str = "set-accept-key";
    const CANCEL_KEY_ASSIGNMENT: &str = "cancel-key-assignment";
    const OPEN_CONFIG: &str = "open-config";
    const RELOAD_CONFIG: &str = "reload-config";
    const EXPORT_AUTOCOMPLETE_COUNTERS: &str = "export-autocomplete-counters";
    const IMPORT_AUTOCOMPLETE_COUNTERS: &str = "import-autocomplete-counters";
    const TOGGLE_AUTOSTART: &str = "toggle-autostart";
    const TOGGLE_LOGGING: &str = "toggle-logging";
    const OPEN_LOG_DIRECTORY: &str = "open-log-directory";
    const CHOOSE_LOG_DIRECTORY: &str = "choose-log-directory";
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
        cancel_key_assignment: MenuItem,
        autostart: CheckMenuItem,
        logging: CheckMenuItem,
        log_file: MenuItem,
    }

    impl MenuItems {
        fn update(&self, view: &MenuView) {
            self.enabled.set_checked(view.enabled);
            self.autocorrect.set_checked(view.autocorrect);
            self.autocomplete.set_checked(view.autocomplete);
            self.autocomplete.set_enabled(view.autocomplete_available);
            self.terminal_auto.set_checked(view.terminal_auto);
            self.autostart.set_checked(view.autostart);
            self.logging.set_checked(view.logging_enabled);
            self.log_file.set_text(&view.log_file_text);
            self.hotkey.set_text(&view.hotkey_text);
            self.hotkey.set_enabled(view.capture_actions_enabled);
            self.accept_key.set_text(&view.accept_key_text);
            self.accept_key
                .set_enabled(view.capture_actions_enabled && view.autocomplete_available);
            self.cancel_key_assignment
                .set_enabled(!view.capture_actions_enabled);
        }
    }

    struct UiState {
        tray: TrayIcon,
        items: MenuItems,
        icons: LayoutIcons<tray_icon::Icon>,
        layout: Option<Lang>,
    }

    impl UiState {
        /// Swap the icon so the active layout's key is in front.
        fn show_layout(&mut self, layout: Option<Lang>) {
            if layout == self.layout {
                return;
            }
            self.layout = layout;
            let icon = Some(self.icons.get(layout).clone());
            #[cfg(target_os = "macos")]
            let result = self.tray.set_icon_with_as_template(icon, true);
            #[cfg(not(target_os = "macos"))]
            let result = self.tray.set_icon(icon);
            if let Err(error) = result {
                tracing::warn!("could not update tray icon: {error}");
            }
            if let Err(error) = self.tray.set_tooltip(Some(tooltip(layout))) {
                tracing::warn!("could not update tray tooltip: {error}");
            }
        }
    }

    fn tooltip(layout: Option<Lang>) -> String {
        format!("MX Keyboard Switcher · {}", layout_text(layout))
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
                view.autocomplete_available,
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
                view.capture_actions_enabled && view.autocomplete_available,
                None,
            );
            let cancel_key_assignment = MenuItem::with_id(
                CANCEL_KEY_ASSIGNMENT,
                "Cancel key assignment",
                !view.capture_actions_enabled,
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
            let logging = CheckMenuItem::with_id(
                TOGGLE_LOGGING,
                "Diagnostic file logging",
                true,
                view.logging_enabled,
                None,
            );
            let log_file = MenuItem::with_id(OPEN_LOG_DIRECTORY, &view.log_file_text, true, None);
            let choose_log_directory =
                MenuItem::with_id(CHOOSE_LOG_DIRECTORY, "Choose log folder…", true, None);
            let quit = MenuItem::with_id(QUIT, "Quit", true, None);

            menu.append_items(&[
                &enabled,
                &autocorrect,
                &autocomplete,
                &terminal_auto,
                &PredefinedMenuItem::separator(),
                &hotkey,
                &accept_key,
                &cancel_key_assignment,
                &PredefinedMenuItem::separator(),
                &open_config,
                &reload_config,
                &export_autocomplete_counters,
                &import_autocomplete_counters,
                &PredefinedMenuItem::separator(),
                &logging,
                &log_file,
                &choose_log_directory,
                &PredefinedMenuItem::separator(),
                &autostart,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .context("failed to build tray menu")?;

            let icons = decode_tray_icons(|rgba, width, height| {
                tray_icon::Icon::from_rgba(rgba, width, height)
                    .map_err(|error| anyhow!("failed to create tray icon: {error}"))
            })?;
            let layout = self.status.layout;
            let builder = TrayIconBuilder::new()
                .with_id("mx-keyboard-switcher")
                .with_menu(Box::new(menu))
                .with_icon(icons.get(layout).clone())
                .with_tooltip(tooltip(layout))
                .with_menu_on_left_click(true);
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_as_template(true);
            let tray = builder.build().context("failed to create native tray")?;

            Ok(UiState {
                tray,
                icons,
                layout,
                items: MenuItems {
                    enabled,
                    autocorrect,
                    autocomplete,
                    terminal_auto,
                    hotkey,
                    accept_key,
                    cancel_key_assignment,
                    autostart,
                    logging,
                    log_file,
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
                    if event.id.0 == CHOOSE_LOG_DIRECTORY {
                        if let Some(directory) = super::choose_log_directory(&self.status.log_path)
                        {
                            let _ = self.cmd_tx.send(Command::SetLogDirectory(directory));
                        }
                    } else if let Some(command) = command_for_menu_id(&event.id) {
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
                    if let Some(ui) = &mut self.ui {
                        ui.items.update(&MenuView::from(&self.status));
                        ui.show_layout(self.status.layout);
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
            CANCEL_KEY_ASSIGNMENT => Some(Command::CancelKeyAssignment),
            OPEN_CONFIG => Some(Command::OpenConfig),
            RELOAD_CONFIG => Some(Command::ReloadConfig),
            EXPORT_AUTOCOMPLETE_COUNTERS => Some(Command::ExportAutocompleteCounters),
            IMPORT_AUTOCOMPLETE_COUNTERS => Some(Command::ImportAutocompleteCounters),
            TOGGLE_AUTOSTART => Some(Command::ToggleAutostart),
            TOGGLE_LOGGING => Some(Command::ToggleLogging),
            OPEN_LOG_DIRECTORY => Some(Command::OpenLogDirectory),
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
    use super::{decode_tray_icon, decode_tray_icons, LayoutIcons, MenuView};
    use crate::engine::Status;
    use mxks_core::layout::Lang;

    fn status(capturing: bool) -> Status {
        Status {
            enabled: false,
            autocorrect: true,
            hotkey: "Pause".into(),
            capturing,
            autocomplete: false,
            autocomplete_available: true,
            terminal_auto: true,
            accept_key: "Tab".into(),
            autostart: true,
            logging_enabled: false,
            log_path: std::env::temp_dir().join("mxks.log"),
            layout: Some(Lang::En),
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
        assert!(!normal.logging_enabled);
        assert_eq!(normal.log_file_text, "Open log folder");
        assert_eq!(normal.hotkey_text, "Change hotkey (now: Pause)");
        assert_eq!(normal.accept_key_text, "Change accept key (now: Tab)");
        assert!(normal.capture_actions_enabled);

        let capturing = MenuView::from(&status(true));
        assert_eq!(capturing.hotkey_text, "Press a key…");
        assert_eq!(capturing.accept_key_text, "Press a key…");
        assert!(!capturing.capture_actions_enabled);
    }

    #[test]
    fn embedded_icons_are_32x32_rgba() {
        // Both sets, so a broken macOS template also fails on other hosts.
        let pngs: [&[u8]; 6] = [
            include_bytes!("../assets/tray-en.png"),
            include_bytes!("../assets/tray-ru.png"),
            include_bytes!("../assets/tray-none.png"),
            include_bytes!("../assets/tray-en-template.png"),
            include_bytes!("../assets/tray-ru-template.png"),
            include_bytes!("../assets/tray-none-template.png"),
        ];
        for png in pngs {
            let (rgba, width, height) = decode_tray_icon(png).expect("embedded icon");
            assert_eq!((width, height), (32, 32));
            assert_eq!(rgba.len(), 32 * 32 * 4);
            let (pixels, remainder) = rgba.as_chunks::<4>();
            assert!(remainder.is_empty());
            assert!(pixels.iter().any(|pixel| pixel[3] == 0));
            assert!(pixels.iter().any(|pixel| pixel[3] != 0));
        }
    }

    #[test]
    fn tray_icon_follows_active_layout() {
        let icons = decode_tray_icons(|rgba, _, _| Ok(rgba)).expect("embedded icons");
        assert_ne!(icons.get(Some(Lang::En)), icons.get(Some(Lang::Ru)));
        assert_ne!(icons.get(Some(Lang::En)), icons.get(None));
        assert_ne!(icons.get(Some(Lang::Ru)), icons.get(None));

        let names = LayoutIcons {
            en: "en",
            ru: "ru",
            other: "other",
        };
        assert_eq!(*names.get(Some(Lang::En)), "en");
        assert_eq!(*names.get(Some(Lang::Ru)), "ru");
        assert_eq!(*names.get(None), "other");
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
            ("cancel-key-assignment", Command::CancelKeyAssignment),
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
            ("toggle-logging", Command::ToggleLogging),
            ("open-log-directory", Command::OpenLogDirectory),
            ("quit", Command::Quit),
        ];
        for (id, expected) in cases {
            assert_eq!(command_for_menu_id(&MenuId::new(id)), Some(expected));
        }
        assert_eq!(command_for_menu_id(&MenuId::new("unknown")), None);
    }
}
