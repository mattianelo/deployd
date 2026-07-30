# Deployd

![Version](https://img.shields.io/badge/version-2.3.2-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0--only-green)
[![deployd](https://snapcraft.io/deployd/badge.svg)](https://snapcraft.io/deployd)

A Linux-native mod manager for Bethesda, REDEngine, Aurora, and Eclipse games, built with GTK4,
libadwaita, and Rust.

> **v2.3.2** If you find bugs, please [open an issue](https://gitlab.com/mattianelo/deployd/-/issues).

Feature overview: [Deployd GitLab Page](https://mattianelo.gitlab.io/deployd/)

---

## Install

The Snap package is available from the Snap Store:

```bash
sudo snap install deployd
```

You can also install from the [Deployd Snap Store page](https://snapcraft.io/deployd).
The Snap package uses the core24 runtime and currently targets 64-bit x86 (`amd64`) Linux systems.
The Nexus Mods page remains available for users who prefer that distribution channel:
[Deployd on Nexus Mods](https://www.nexusmods.com/skyrimspecialedition/mods/174218).

---

## Supported Games

| Engine | Games |
|--------|-------|
| Bethesda | Skyrim Special Edition · Fallout 4 · Fallout: New Vegas · Starfield |
| REDEngine | The Witcher 3 · Cyberpunk 2077 · The Witcher 2 |
| Aurora | The Witcher 1 |
| Eclipse | Dragon Age: Origins |

---

## Features

- **Game Setup Wizard** — A first-run wizard guides you through selecting your games and pointing Deployd to their installation folder and Wine prefix
- **Nexus Mods Integration** — SSO login, NXM deep links, one-click update checking,
  and manual Nexus Mod ID correction from Mod Properties
- **FOMOD Installer** — Full wizard with conditional steps, image previews, and DLC-aware auto-selection
- **Mod Profiles** — Per-game profiles to switch between configurations instantly
- **Plugin Load Order** — Select Mode reorder workflow for `.esp`/`.esm`/`.esl` management written to `plugins.txt`
- **Conflict Detection** — Per-file visibility into which mods override each other, with a detailed Conflicts section in each mod's Properties dialog (The Witcher 1's Override/ files are matched by filename regardless of subfolder depth)
- **Priority-Based Deployment** — Hardlink deployment; lower in the list wins conflicts
- **Tool Launcher** — Run xEdit, LOOT, BodySlide and more through the package-managed Windows runtime
- **Save Management** — Browse and back up game saves associated with the active profile,
  including each Witcher game's distinct Documents folder
- **Mod Notes** — Attach personal notes to any mod; preview on hover from the list
- **Notifications Panel** — External changes and alerts collected in a sidebar, with "All Caught Up" state
- **Download pause/resume** — Active downloads can be paused and resumed mid-transfer
- **Responsive work feedback** — Long archive, scan, deploy, and runtime setup tasks show clear
  busy messages while keeping the interface responsive; Downloads rows mirror install phases and
  Nexus metadata work without leaving completed downloads or installs stuck as active; External
  Tools launches stay cancellable through the running Deployd-owned process
- **GNOME HIG interface** — Libadwaita navigation, adaptive split views, toolbar views,
  status pages, toast feedback, dialogs, list rows, and appearance settings keep Deployd aligned
  with modern GNOME app conventions

---

## Interface

Deployd uses libadwaita throughout its primary workflows:

- The main Mod Order and Plugin Order panels use adaptive libadwaita navigation, so wide windows
  show both panes side by side while narrow windows collapse cleanly.
- Select Mode is the v2 reorder workflow for Mod Order and Plugin Order. Enter Select Mode before
  selecting rows, dragging items, or changing load order. Dragging a selected plugin moves the
  selected plugin block together.
- Downloads, notifications, profile actions, deploy actions, snapshots, and Nexus account controls
  use GNOME-style rows, popovers, status pages, and toast feedback.
- Install-related workflows, including FOMOD, Pre-install, Absorb External Changes, Mod Properties,
  Tool Manager, Game Setup, and the Welcome Wizard, use libadwaita toolbar, clamp, preferences,
  action-row, and alert patterns.
- Mod, plugin, and download lists use balanced libadwaita row layouts. The old compact row mode was
  removed in favor of one consistent middle-density presentation.

---

## Storage

### Cache Folder

By default Deployd stores all cached mod files in `~/.local/share/deployd/cache/` (or `$SNAP_USER_COMMON/deployd/cache/` in the Snap). You can relocate a game's cache to any directory via **Settings → Manage Games**, under the "Cache Folder" row for that game.

**Why would you move it?**  
If your game lives on a secondary drive, placing the cache on the same drive eliminates the copy overhead on every install: Deployd deploys mods using **hardlinks** (zero-copy, zero extra disk space).

In the Snap package, Steam-managed game folders can be exposed through a separate mount. When Linux rejects a hardlink across that boundary, Deployd falls back to copying the file while keeping it tracked as Deployd-managed.

Strict Snap confinement still controls which folders the app can see. Deployd validates selected
game, Wine prefix, downloads, cache, and migration-import folders before saving them, and explains
blocked hidden-home paths, ungranted removable media, or read/write access failures immediately.
Folders selected through the desktop portal remain available across restarts, including folders on
external drives. When download records point at a document-portal mount, Deployd resolves the
original host path before moving the archive to Trash.

AppImage-to-Snap game migration preserves installed-mod source metadata, download metadata, and
the profile associated with currently deployed files. AppImage archive paths are cleared because
they are not valid inside the Snap; rescanning the Snap downloads folder reattaches those archives.

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

> AppImage startup registers Deployd as the NXM link handler and repairs stale registration
> automatically. Once authenticated, clicking **Mod Manager Download** on Nexus Mods sends files
> directly to your download queue.

### 3. Installing Mods

- **Local archive** — Drag-and-drop a `.zip`, `.7z`, or `.rar` onto the mod list, or use the **+** button
- **From Nexus Mods** — Click **Mod Manager Download**; the file downloads into Deployd automatically
- **FOMOD mods** — A wizard opens automatically if the archive includes a FOMOD installer
- **File selection** — The Install Mod dialog lets you deselect archive entries before they are
  cached and deployed
- **Reinstall** — Use the ↺ button on any mod row to re-extract from its original archive, or use the refresh button in the Downloads panel
- **Replace** — When a name conflict occurs during install, choose Replace to swap the mod in-place (preserving load order position)

### 4. Deployment

Mods are deployed as **hardlinks** into the game's Data directory — originals stay safely in Deployd's cache.

- Toggle individual mods on/off with the switch in each row
- Enter **Select Mode** to reorder mods; in v2, Select Mode is the only way to modify load order
- Drag selected rows to reorder; mods lower in the list win file conflicts
- **Deploy** — applies your changes to the game folder
- **Purge** — removes all Deployd-managed files (only tracked hardlinks; your game files are safe)

### 5. Plugin Load Order

For Bethesda games, manage plugins separately in the **Plugins** tab:

- Enter **Select Mode** to modify plugin load order
- Drag selected plugins to set load order
- Toggle plugins on/off individually
- Load order is written to `plugins.txt` on deploy

### 6. Profiles

- Click the **profile selector** in the toolbar to create or switch profiles
- Each profile saves which mods are enabled, their priority order, and the full plugin load order
- Opening a game restores the profile used by its most recent successful deployment, and rapid
  game changes cannot apply a delayed profile load from another game
- Switching profiles re-deploys automatically
- Snap installations report expired folder grants and ask for the affected game or Wine-prefix
  folder to be reselected instead of presenting inaccessible official plugins as missing

### 7. Save Management

The **Saves** tab lists game saves associated with your active profile. Informational only — Deployd never modifies save files.

### 8. External Tools

Launch modding tools through the package-managed runtime from the **Tools** panel:

- Tools inside the game's Proton prefix are detected automatically
- Add tools manually via the tool manager
- Each tool supports custom arguments and a working directory
- BodySlide launches prefer the 64-bit executable when both `BodySlide.exe` and
  `BodySlide x64.exe` are present
- Closing a tool scans for external changes, runs LOOT for supported games, and deploys automatically
- In the Snap package, tools run through the Wine platform content snaps; if either content plug is
  disconnected, Deployd shows the exact `snap connect` command with the provider slot included
- The Snap package owns the `io.mattianelo.deployd` session D-Bus name for
  single-instance activation and NXM link forwarding

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

*Built in 🇦🇷 with [Rust](https://www.rust-lang.org) · [GTK4](https://gtk.org) · [Relm4](https://relm4.org) · [SQLite](https://sqlite.org) · and the help of agentic AI workflows*

---

GPL-3.0-only
