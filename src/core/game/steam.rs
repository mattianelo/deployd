use std::path::PathBuf;

/// Find the Steam installation root, checking the four canonical locations for
/// native deb, legacy symlink, Snap, and Flatpak Steam variants.
///
/// Returns the first path whose `steamapps/` subdirectory exists, or `None`
/// if Steam is not detected.  The result is used to set
/// `STEAM_COMPAT_CLIENT_INSTALL_PATH` for the Proton runtime.
pub fn find_steam_root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join("snap/steam/common/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .find(|p| p.join("steamapps").is_dir())
}
