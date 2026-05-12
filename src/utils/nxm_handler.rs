use std::env;
use std::fs;
use std::process::Command;

/// Register deployd as the system NXM protocol handler.
///
/// Writes a `.desktop` file with the correct `Exec=` path to
/// `~/.local/share/applications/` and calls `xdg-mime` and
/// `update-desktop-database`. All failures are silent — missing tools
/// or write errors must not crash the application.
///
/// This mirrors what the AppImage `AppRun` does on first run, but runs
/// unconditionally so that any installation method (source build, AppImage,
/// Snap dev build) keeps the handler current.
pub fn ensure_registered() {
    // Snap manages NXM registration via snapd + the bundled .desktop file.
    // AppImage's AppRun script handles registration on first run.
    // Running here in either package context would overwrite the other's handler.
    if env::var("SNAP").is_ok() || env::var("APPIMAGE").is_ok() {
        return;
    }

    let Ok(exe) = env::current_exe() else {
        return;
    };
    let Ok(home) = env::var("HOME") else {
        return;
    };
    let apps_dir = std::path::Path::new(&home).join(".local/share/applications");
    if fs::create_dir_all(&apps_dir).is_err() {
        return;
    }

    let desktop_content = format!(
        "[Desktop Entry]\n\
         Name=Deployd\n\
         Comment=Mod manager for PC games\n\
         Exec={exe} %u\n\
         Icon=deployd\n\
         Terminal=false\n\
         Type=Application\n\
         Categories=Game;Utility;\n\
         Keywords=mod;manager;\n\
         MimeType=x-scheme-handler/nxm;\n\
         StartupWMClass=deployd\n",
        exe = exe.display()
    );

    let desktop_path = apps_dir.join("io.mattianelo.deployd.desktop");
    if fs::write(&desktop_path, &desktop_content).is_err() {
        return;
    }

    let _ = Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = Command::new("xdg-mime")
        .args([
            "default",
            "io.mattianelo.deployd.desktop",
            "x-scheme-handler/nxm",
        ])
        .status();
}
