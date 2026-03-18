# Deployd

![Version](https://img.shields.io/badge/version-0.9.6-blue)
![Status](https://img.shields.io/badge/status-beta-orange)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0-green)

A Linux-native mod manager for Bethesda and REDEngine games, built with GTK4 and Rust.
Works with both **Steam** and **Heroic Launcher** out of the box.

> **Public Beta — v0.9.6.** If you find bugs, please [open an issue](https://gitlab.com/mattianelo/deployd/-/issues).

---

## Supported Games

| Engine | Games |
|--------|-------|
| Bethesda | Skyrim Special Edition · Fallout 4 · Fallout: New Vegas · Starfield |
| REDEngine | The Witcher 3 · Cyberpunk 2077 |
| Eclipse | Dragon Age: Origins *(Experimental)* |

---

## Features

- **Game Detection** — Auto-detects your library from Steam and Heroic Launcher (GOG/Epic)
- **Nexus Mods Integration** — SSO login, NXM deep links, and one-click update checking
- **FOMOD Installer** — Full wizard with conditional steps, image previews, and DLC-aware auto-selection
- **Mod Profiles** — Per-game profiles to switch between configurations instantly
- **Plugin Load Order** — Drag-and-drop `.esp`/`.esm`/`.esl` management written to `plugins.txt`
- **Conflict Detection** — Per-file visibility into which mods override each other
- **Priority-Based Deployment** — Hardlink deployment; lower in the list wins conflicts
- **Tool Launcher** — Run xEdit, LOOT, BodySlide and more through Wine/Proton
- **Save Management** — Browse game saves associated with the active profile
- **Mod Notes** — Attach personal notes to any mod; preview on hover from the list
- **Notifications Panel** — External changes and alerts collected in a sidebar, with "All Caught Up" state
- **Dragon Age: Origins (Experimental)** — Override mods and `.dazip` archives supported; enable in Settings → Games

---

## Installation

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

## Getting Started

### 1. Game Detection

Deployd reads your game library automatically from **Steam** and **Heroic Launcher**.

- For Steam: games are detected from the default Steam library
- For Heroic: make sure the game has been launched at least once so Heroic has written its config

Supported titles: **Skyrim SE**, **Fallout 4**, **Fallout: New Vegas**, **Starfield**, **The Witcher 3**, **Cyberpunk 2077**, **Dragon Age: Origins** *(Experimental)*

Use the game selector in the top bar to switch between detected games.

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
- **Reinstall** — Use the refresh button in the Downloads panel to re-extract and replace an existing mod
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
