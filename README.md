# Deployd

![Version](https://img.shields.io/badge/version-1.1.1-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0-green)

A Linux-native mod manager for Bethesda, REDEngine, Aurora, and Eclipse games, built with GTK4 and Rust.

> **v1.1.1** If you find bugs, please [open an issue](https://gitlab.com/mattianelo/deployd/-/issues).

---

## Supported Games

| Engine | Games |
|--------|-------|
| Bethesda | Skyrim Special Edition · Fallout 4 · Fallout: New Vegas · Starfield |
| REDEngine | The Witcher 3 · Cyberpunk 2077 |
| Aurora | The Witcher 1 |
| Eclipse | Dragon Age: Origins |

---

## Features

- **Game Setup Wizard** — A first-run wizard guides you through selecting your games and pointing Deployd to their installation folder and Wine prefix
- **Nexus Mods Integration** — SSO login, NXM deep links, and one-click update checking
- **FOMOD Installer** — Full wizard with conditional steps, image previews, and DLC-aware auto-selection
- **Mod Profiles** — Per-game profiles to switch between configurations instantly
- **Plugin Load Order** — Drag-and-drop `.esp`/`.esm`/`.esl` management written to `plugins.txt`
- **Conflict Detection** — Per-file visibility into which mods override each other, with a detailed Conflicts section in each mod's Properties dialog (The Witcher 1's Override/ files are matched by filename regardless of subfolder depth)
- **Priority-Based Deployment** — Hardlink deployment; lower in the list wins conflicts
- **Tool Launcher** — Run xEdit, LOOT, BodySlide and more through Wine/Proton
- **Save Management** — Browse game saves associated with the active profile
- **Mod Notes** — Attach personal notes to any mod; preview on hover from the list
- **Notifications Panel** — External changes and alerts collected in a sidebar, with "All Caught Up" state
- **Download pause/resume** — Active downloads can be paused and resumed mid-transfer
- **Compact mode & color scheme** — Appearance settings let you switch color scheme and enable compact rows for the mod and plugin lists

---

## Installation

### Snap

```bash
snap install deployd
```

The Snap bundles all required libraries and UMU Launcher. No additional runtime dependencies needed.

> **Note:** The NXM scheme handler requires the `network-bind` interface. Connect it once after install:
> ```bash
> snap connect deployd:network-bind
> ```

### AppImage

### Runtime Dependencies (AppImage)

The AppImage bundles most of its dependencies, but the following must be present on your system:

**Fedora / RHEL:**
```bash
sudo dnf install gtk4 libadwaita
```

**Ubuntu / Debian:**
```bash
sudo apt install libgtk-4-1 libadwaita-1-0
```

---

## Storage

### Cache Folder

By default Deployd stores all cached mod files in `~/.local/share/deployd/cache/` (or `$SNAP_USER_COMMON/deployd/cache/` in the Snap). You can relocate a game's cache to any directory via **Settings → Manage Games**, under the "Cache Folder" row for that game.

**Why would you move it?**  
If your game lives on a secondary drive, placing the cache on the same drive eliminates the copy overhead on every install: Deployd deploys mods using **hardlinks** (zero-copy, zero extra disk space).

**Hardlink filesystem constraint**

Hardlinks require both the cache directory and the game directory to reside on the **same filesystem** — that is, they must share the same `st_dev` value as reported by the OS. Concretely:

| Storage setup | What counts as "same filesystem" |
|---|---|
| Standard partitions | Same partition / block device |
| BTRFS | Same **subvolume** — hardlinks cannot cross subvolume boundaries even on the same physical disk, because each subvolume has its own inode space |
| ZFS | Same **dataset** — hardlinks cannot cross datasets even within the same pool |
| LVM / LUKS | Same logical volume |

If you select a cache directory on a different filesystem than the game folder, Deployd will reject the selection with a clear error message. No files are moved until the check passes.

---

## Getting Started

### 1. Game Setup

On first launch, Deployd shows a **Welcome Wizard** that walks you through adding your games:

1. Select which games you want to manage from the list
2. For each game, browse to its **Installation Folder**
3. For each game, browse to its **Wine Prefix** (the Proton or Wine directory used to run the game)
4. Click **Finish** — your games are saved and ready to use

Both the installation folder and Wine prefix are required for each game. To add or remove games later, go to **Settings → Manage Games**.

Supported titles: **Skyrim SE**, **Fallout 4**, **Fallout: New Vegas**, **Starfield**, **The Witcher 3**, **Cyberpunk 2077**, **The Witcher 1**, **Dragon Age: Origins**

Use the game selector in the top bar to switch between managed games.

### 2. Nexus Mods Login

Open **Settings** (gear icon) and log in under **Nexus Mods**:

1. Click **Login with Nexus Mods** — your browser opens, you authorize, done.
2. *(Optional)* Prefer a manual API key? Enter it directly under **Advanced → API Key**.
   - Get your key from [nexusmods.com → Settings → API](https://www.nexusmods.com/users/myaccount?tab=api)

> Once authenticated, Deployd registers itself as the NXM link handler. Clicking **Mod Manager Download** on Nexus Mods sends files directly to your download queue.

### 3. Installing Mods

- **Local archive** — Drag-and-drop a `.zip`, `.7z`, or `.rar` onto the mod list, or use the **+** button
- **From Nexus Mods** — Click **Mod Manager Download**; the file downloads into Deployd automatically
- **FOMOD mods** — A wizard opens automatically if the archive includes a FOMOD installer
- **Reinstall** — Use the ↺ button on any mod row to re-extract from its original archive, or use the refresh button in the Downloads panel
- **Replace** — When a name conflict occurs during install, choose Replace to swap the mod in-place (preserving load order position)

### 4. Deployment

Mods are deployed as **hardlinks** into the game's Data directory — originals stay safely in Deployd's cache.

- Toggle individual mods on/off with the switch in each row
- Drag rows to reorder; mods lower in the list win file conflicts
- **Deploy** — applies your changes to the game folder
- **Purge** — removes all Deployd-managed files (only tracked hardlinks; your game files are safe)

### 5. Plugin Load Order

For Bethesda games, manage plugins separately in the **Plugins** tab:

- Drag-and-drop to set load order
- Toggle plugins on/off individually
- Load order is written to `plugins.txt` on deploy

### 6. Profiles

- Click the **profile selector** in the toolbar to create or switch profiles
- Each profile saves which mods are enabled, their priority order, and the full plugin load order
- Switching profiles re-deploys automatically

### 7. Save Management

The **Saves** tab lists game saves associated with your active profile. Informational only — Deployd never modifies save files.

### 8. External Tools

Launch modding tools through Wine/Proton from the **Tools** panel:

- Tools inside the game's Proton prefix are detected automatically
- Add tools manually via the tool manager
- Each tool supports custom arguments and a working directory

---

## How It Works

1. **Cache** — Archives are extracted into a per-mod cache directory with normalised paths
2. **Deploy** — Files are hardlinked from cache into the game's Data directory, with per-game path rules applied
3. **Track** — Every deployed file is recorded in SQLite; purge removes only tracked links
4. **Profiles** — Full mod + plugin state is saved and restored per profile

---

## Support

If Deployd saves you time, consider supporting development:

[![Support on Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/mattianelo)

---

*Built in 🇦🇷 with [Rust](https://www.rust-lang.org) · [GTK4](https://gtk.org) · [Relm4](https://relm4.org) · [SQLite](https://sqlite.org) · and the help of [Claude](https://claude.ai)*

---

GPL-3.0-only
