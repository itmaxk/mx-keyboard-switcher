//! Autostart ("start at login") management.
//!
//! The OS entry itself is the source of truth — nothing is stored in the
//! config file. The entries match what the install scripts create
//! (`scripts/install.sh`, `scripts/install.ps1`), so this module toggles the
//! same record the installer set up:
//!
//! - Linux:   `~/.config/autostart/mx-keyboard-switcher.desktop`
//! - macOS:   `~/Library/LaunchAgents/com.itmaxk.mx-keyboard-switcher.plist`
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
//!   value `MXKeyboardSwitcher`

use anyhow::Result;

/// Whether an autostart entry for this app currently exists.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Create or remove the autostart entry, pointing at the running executable.
pub fn set_enabled(on: bool) -> Result<()> {
    imp::set_enabled(on)
}

/// Remove a file, treating "already gone" as success.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn remove_if_exists(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("remove {}: {e}", path.display())),
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    fn desktop_file() -> Option<PathBuf> {
        Some(
            dirs::config_dir()?
                .join("autostart")
                .join("mx-keyboard-switcher.desktop"),
        )
    }

    fn desktop_entry(exe: &Path) -> String {
        // Quoted Exec per the Desktop Entry spec, in case the path has spaces.
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=MX Keyboard Switcher\n\
             Exec=\"{}\"\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        )
    }

    fn enable_at(path: &Path, exe: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        std::fs::write(path, desktop_entry(exe))
            .with_context(|| format!("write {}", path.display()))
    }

    pub fn is_enabled() -> bool {
        desktop_file().is_some_and(|p| p.exists())
    }

    pub fn set_enabled(on: bool) -> Result<()> {
        let path = desktop_file().context("no config directory")?;
        if on {
            let exe = std::env::current_exe().context("resolve current executable")?;
            enable_at(&path, &exe)
        } else {
            super::remove_if_exists(&path)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn desktop_entry_roundtrip() {
            let dir =
                std::env::temp_dir().join(format!("mxks-autostart-test-{}", std::process::id()));
            let path = dir.join("autostart").join("mx-keyboard-switcher.desktop");
            let exe = Path::new("/some dir/mx-keyboard-switcher");

            enable_at(&path, exe).unwrap();
            assert!(path.exists());
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(body.contains("[Desktop Entry]"));
            assert!(body.contains("Exec=\"/some dir/mx-keyboard-switcher\""));

            crate::autostart::remove_if_exists(&path).unwrap();
            assert!(!path.exists());
            // Removing again is still fine.
            crate::autostart::remove_if_exists(&path).unwrap();

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    const LABEL: &str = "com.itmaxk.mx-keyboard-switcher";

    fn plist_file() -> Option<PathBuf> {
        Some(
            dirs::home_dir()?
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LABEL}.plist")),
        )
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn plist(exe: &Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
      <key>SuccessfulExit</key>
      <false/>
    </dict>
  </dict>
</plist>
"#,
            xml_escape(&exe.display().to_string())
        )
    }

    fn launchctl(args: &[&str], path: &Path) -> std::io::Result<std::process::ExitStatus> {
        std::process::Command::new("launchctl")
            .args(args)
            .arg(path)
            .status()
    }

    pub fn is_enabled() -> bool {
        plist_file().is_some_and(|p| p.exists())
    }

    pub fn set_enabled(on: bool) -> Result<()> {
        let path = plist_file().context("no home directory")?;
        if on {
            let exe = std::env::current_exe().context("resolve current executable")?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create {}", dir.display()))?;
            }
            std::fs::write(&path, plist(&exe))
                .with_context(|| format!("write {}", path.display()))?;
            // Reload in case an older copy was already registered.
            let _ = launchctl(&["unload"], &path);
            launchctl(&["load", "-w"], &path).context("run launchctl load")?;
            Ok(())
        } else {
            let _ = launchctl(&["unload"], &path);
            super::remove_if_exists(&path)
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use anyhow::{Context, Result};
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "MXKeyboardSwitcher";

    pub fn is_enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
            .is_ok()
    }

    pub fn set_enabled(on: bool) -> Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(RUN_KEY)
            .context("open Run registry key")?;
        if on {
            let exe = std::env::current_exe().context("resolve current executable")?;
            key.set_value(VALUE_NAME, &format!("\"{}\"", exe.display()))
                .context("set Run registry value")?;
        } else {
            match key.delete_value(VALUE_NAME) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("delete Run registry value"),
            }
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use anyhow::{bail, Result};

    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_on: bool) -> Result<()> {
        bail!("autostart is not supported on this platform")
    }
}
