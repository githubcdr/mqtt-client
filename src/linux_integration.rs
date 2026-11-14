//! Best-effort XDG desktop integration for Linux.
//!
//! Wayland compositors and modern desktop shells (KDE, GNOME, …) ignore
//! runtime window icons when decorating taskbars/docks: they resolve the
//! icon and app name by matching the client's `app_id` (`mqtt-client`)
//! against `mqtt-client.desktop` and then looking up `Icon=mqtt-client`
//! in the hicolor icon theme. To make the portable single binary show
//! its real icon, on startup we install into `$XDG_DATA_HOME` (default
//! `~/.local/share`):
//!
//! - `applications/mqtt-client.desktop` (with `Exec=` pointing at the
//!   currently running executable)
//! - `icons/hicolor/scalable/apps/mqtt-client.svg` plus PNG renditions
//!   at common sizes
//!
//! The registration is idempotent: files are only written when missing
//! or outdated (e.g. the binary moved, or the app upgraded). Failures
//! are always ignored — this must never affect launching the app.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::icon::{encode_png_rgba, render_icon_rgba};

const APP_ID: &str = "mqtt-client";
const ICON_SVG: &str = include_str!("../assets/icon.svg");
const PNG_SIZES: [usize; 6] = [32, 48, 64, 128, 256, 512];

/// Install the desktop entry + hicolor icons. Call once before the GUI starts.
/// Returns the installed desktop entry path when integration succeeded.
pub fn ensure_desktop_entry() -> Option<PathBuf> {
    let data_home = xdg_data_home()?;
    let exe = match std::env::current_exe() {
        Ok(path) => path.canonicalize().unwrap_or(path),
        Err(_) => return None,
    };

    let mut artwork_refresh = false;

    // 1. Desktop entry (re-written when the Exec path or app version changes).
    let desktop_path = data_home
        .join("applications")
        .join(format!("{APP_ID}.desktop"));
    let desktop_contents = desktop_entry(&exe);
    if fs::read_to_string(&desktop_path).ok().as_deref() != Some(desktop_contents.as_str()) {
        write_file(&desktop_path, desktop_contents.as_bytes());
        artwork_refresh = true;
    }

    // 2. Scalable SVG icon (re-written when the embedded artwork changes).
    let icons_root = data_home.join("icons").join("hicolor");
    let svg_path = icons_root
        .join("scalable")
        .join("apps")
        .join(format!("{APP_ID}.svg"));
    if fs::read_to_string(&svg_path).ok().as_deref() != Some(ICON_SVG) {
        write_file(&svg_path, ICON_SVG.as_bytes());
        artwork_refresh = true;
    }

    // 3. PNG fallbacks — rendered when missing, and re-rendered on refresh.
    let pngs_missing = PNG_SIZES
        .iter()
        .any(|size| !png_path(&icons_root, *size).is_file());
    if artwork_refresh || pngs_missing {
        for &size in &PNG_SIZES {
            let path = png_path(&icons_root, size);
            if !artwork_refresh && path.is_file() {
                continue;
            }
            let rgba = render_icon_rgba(size);
            let png = encode_png_rgba(size as u32, size as u32, &rgba);
            let _ = write_file(&path, &png);
        }
        refresh_caches(&data_home);
    }

    if desktop_path.is_file() {
        Some(desktop_path)
    } else {
        None
    }
}

fn desktop_entry(exe: &Path) -> String {
    let version = env!("CARGO_PKG_VERSION");
    // Quote the target only if required, for maximal parser compatibility.
    let exec_target = if exe.display().to_string().contains(' ') {
        format!("\"{}\"", exe.display())
    } else {
        exe.display().to_string()
    };
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=MQTT Explorer RS\n\
         GenericName=MQTT Client\n\
         Comment=Lightweight MQTT 3.1 / 3.1.1 / 5.0 GUI Client with TLS & Topic Explorer\n\
         Exec={exec_target}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Development;Network;Utility;\n\
         Keywords=MQTT;IoT;Broker;PubSub;Telemetry;\n\
         StartupWMClass={APP_ID}\n\
         \n\
         # Generated automatically by mqtt-client v{version} on launch.\n"
    )
}

fn png_path(icons_root: &Path, size: usize) -> PathBuf {
    icons_root
        .join(format!("{size}x{size}"))
        .join("apps")
        .join(format!("{APP_ID}.png"))
}

fn xdg_data_home() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(value.trim());
        if !value.trim().is_empty() && path.is_absolute() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".local").join("share"))
}

fn write_file(path: &Path, contents: &[u8]) -> bool {
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    fs::write(path, contents).is_ok()
}

fn refresh_caches(data_home: &Path) {
    // Best effort: shells pick up new entries immediately on most systems,
    // but refreshing avoids waiting for a session restart.
    let applications = data_home.join("applications");
    let _ = Command::new("update-desktop-database")
        .arg(&applications)
        .stderr(std::process::Stdio::null())
        .spawn();
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-q", "-t", "-f"])
        .arg(data_home.join("icons").join("hicolor"))
        .stderr(std::process::Stdio::null())
        .spawn();
    // KDE/Plasma resolves app_id -> .desktop -> icon through its own KSycoca
    // database, which only indexes menus on session start unless nudged.
    // Without this, the taskbar shows a generic icon after self-registration
    // until the next session restart.
    let _ = Command::new("kbuildsycoca6")
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_desktop_entry_and_icons_idempotently() {
        let root =
            std::env::temp_dir().join(format!("mqtt-client-xdg-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::env::set_var("XDG_DATA_HOME", &root);

        ensure_desktop_entry();
        let desktop = root.join("applications").join("mqtt-client.desktop");
        assert!(desktop.is_file(), "desktop entry was not installed");
        let contents = fs::read_to_string(&desktop).unwrap();
        assert!(contents.contains("Exec="), "missing Exec line");
        assert!(contents.contains("Icon=mqtt-client"));
        assert!(contents.contains("StartupWMClass=mqtt-client"));

        assert!(root
            .join("icons/hicolor/scalable/apps/mqtt-client.svg")
            .is_file());
        assert!(root
            .join("icons/hicolor/512x512/apps/mqtt-client.png")
            .is_file());
        assert!(root
            .join("icons/hicolor/48x48/apps/mqtt-client.png")
            .is_file());

        // Second run must be a no-op that leaves everything in place.
        ensure_desktop_entry();
        assert!(desktop.is_file());

        std::env::remove_var("XDG_DATA_HOME");
        let _ = fs::remove_dir_all(&root);
    }
}
