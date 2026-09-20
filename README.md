# Deployd

![Version](https://img.shields.io/badge/version-3.0.0-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0--only-green)
[![deployd](https://snapcraft.io/deployd/badge.svg)](https://snapcraft.io/deployd)

A Linux-native mod manager for Mass Effect Legendary Edition, Bethesda, REDEngine, Aurora,
and Eclipse games.

> **v3.0.0** If you find bugs, please [open an issue](https://gitlab.com/mattianelo/deployd/-/issues).

Feature overview: [Deployd GitLab Page](https://mattianelo.gitlab.io/deployd/)

---

## Install

The Snap package is available from the Snap Store:

```bash
sudo snap install deployd
```

You can also install from the [Deployd Snap Store page](https://snapcraft.io/deployd).
The Snap package supports 64-bit x86 (`amd64`) Linux systems.
Application update notifications are shown only by the AppImage; Snap updates are managed by
snapd and the configured Snap Store channel.
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

### Mass Effect Legendary Edition (experimental)

Manage LE1, LE2 and LE3 with separate mod libraries and profiles. Supported mods
include Community Patch, cosmetic and content mods, raw and precompiled M3TO
textures (such as No Headgear for Squadmates, ALOT and ISL), shader mods, squadmate outfits, LE2 DLC configuration
options and LE2 email additions.
Deployd reports unsupported packages before installation.

1. Add a **clean installation** containing all three games and select its Wine prefix
   separately. Deployd treats this installation as your restoration point. Initial
   setup can take several minutes and shows progress for each game.
2. Select a game, install a mod archive and choose its options.
3. Enable and order your mods, then choose **Deploy**, select the game language and
   confirm once. Deployd prepares and applies the profile as one operation.
4. Deploy again after disabling, removing, reordering or switching profiles.
   **Purge** restores managed game files while keeping your mod library. Reinstall
   an archive to change its options or contents.

Place required mods before the mods that need them. Mod list order controls file
replacements; DLC priority is set by mod authors and does not change when you
reorder the list. Texture overrides and shader mods also follow DLC priority.

Required support components download automatically on first use and are retained
for offline reuse. Missing managed components and plugins can be repaired from the
deployment dialog. Interrupted deployments recover after restart. Unexpected
external changes are preserved and must be resolved before deploying again.
Large texture installations can take time; follow the progress shown in Deployd.

ASI/DLL plugins require your approval for each package version and game. Deployd
never runs archive installers or scripts. Follow the mod author's instructions
for any additional Proton DLL overrides.

When an archive contains both a game mod and a launcher component, add it from the
game's mod list. The launcher component belongs to that parent mod: it follows the
parent's enabled state and priority and is applied during the same Deploy. Disabling,
replacing or removing the parent takes effect on the launcher during the next Deploy.
Purge removes the current game's launcher components while preserving components still
deployed by another Legendary Edition game. Deployd records the shared launcher's
original files during game setup and restores them after the last parent mod using them
is removed. Required launcher support remains active while any deployed game needs it.

**Limitations:** MEM textures, headmorphs and portable MELE profile transfers are
not supported. Launcher executable replacements and launcher installer choices are
also unsupported. Restore existing MEM textures to clean game
content before adding the game. Mod deployment does not edit saves.

---

## Features

- **Game Setup Wizard** — A first-run wizard guides you through selecting your games and pointing Deployd to their installation folder and Wine prefix
- **Nexus Mods Integration** — SSO login, NXM deep links, one-click update checking,
  and manual Nexus Mod ID and installed-version correction from Mod Properties
- **FOMOD Installer** — Full wizard with conditional steps, image previews, and DLC-aware auto-selection
- **Mod Profiles** — Per-game editable configurations; selecting a profile does not change game
  files or live saves until you explicitly choose **Deploy**
- **Deployment History** — Every successful Deploy retains the complete profile configuration and
  installed content needed to restore it as a new editable profile, including disabled mods and
  conflict-losing files. History remains after ordinary profile or mod deletion and excludes saves
- **Plugin Load Order** — Select Mode reorder workflow for `.esp`/`.esm`/`.esl` management written to `plugins.txt`
- **Conflict Detection** — Per-file visibility into which mods override each other, with a detailed Conflicts section in each mod's Properties dialog (The Witcher 1's Override/ files are matched by filename regardless of subfolder depth)
- **Priority-Based Deployment** — Lower in the list wins file conflicts; MELE also
  respects the DLC priorities supplied by mod authors
- **Tool Launcher** — Run xEdit, LOOT, BodySlide and more through the package-managed Windows
  runtime; the Snap prepares an isolated per-game tool environment and silently installs its
  verified .NET compatibility runtime on first use
- **Save Management** — Browse and back up game saves associated with the active profile,
  including each Witcher game's distinct Documents folder
- **Mod Notes** — Attach personal notes to any mod; preview on hover from the list
- **Notifications Panel** — External changes and alerts collected in a sidebar, with "All Caught Up" state
- **Download pause/resume** — Active downloads can be paused and resumed mid-transfer
- **Responsive work feedback** — Long archive, scan, deploy, and runtime setup tasks show clear
  busy messages while keeping the interface responsive; Downloads rows mirror install phases and
  Nexus metadata work without leaving completed downloads or installs stuck as active; External
  Tools launches stay cancellable through the running Deployd-owned process
- **Resilient workflows** — Startup and installation failures report actionable errors, while
  callbacks from dialogs that have already closed are discarded without terminating Deployd
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
  selected plugin block together. Batch Enable and Disable actions keep Select Mode active so
  additional rows can be managed before choosing Done, with each row showing its current state.
  Deployment remains unavailable until Select Mode is finished.
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

Deployment history is stored beside each game's configured cache and moves with it through a
recoverable copy-and-verify operation. Historical content uses independent retained copies, so
editing or replacing files in the writable mod cache cannot change an older generation. If an
external cache or its portal grant is unavailable, restoration remains blocked until access is
restored; Deployd does not substitute content from another location.

**Why would you move it?**  
Most supported games use **hardlinks**, which avoid duplicate file storage when the
cache and game share a compatible filesystem. Moving the cache can make that possible
for games on a secondary drive. MELE uses separate copies for rebuilding and requires
additional disk space.

In the Snap package, Steam-managed game folders can be exposed through a separate mount. When Linux rejects a hardlink across that boundary, Deployd falls back to copying the file while keeping it tracked as Deployd-managed.

External Windows tools in the Snap use a durable, isolated Wine environment instead of modifying
the game's Steam, Heroic, or Proton prefix. The first launch displays setup progress while Deployd
downloads, verifies, and silently installs Wine Mono. If Mono is unavailable, native tools can be
launched without it and Deployd retries setup on a later launch. Game settings, saves, and tool
discovery continue to use the configured game prefix.

Strict Snap confinement still controls which folders the app can see. Deployd validates selected
game, Wine prefix, downloads, and cache folders before saving them, and explains
blocked hidden-home paths, ungranted removable media, or read/write access failures immediately.
Folders selected through the desktop portal remain available across restarts, including folders on
external drives. The downloads folder picker always requests this portal access instead of relying
on direct removable-media access. When download records point at a document-portal mount, Deployd
uses the desktop's Trash service to remove the archive.

If the desktop Trash service fails, Deployd asks whether to permanently delete only that archive
or remove its Downloads entry while leaving the archive untouched. It never silently converts a
Trash action into permanent deletion.

If the desktop portal returns an external drive's direct mount path or an inaccessible portal route,
Deployd prompts for the Snap's manual removable-media connection. The displayed command uses the
name of the Snap installation you are running.
Run the command, then select the downloads folder again.

If a saved game folder or Wine prefix becomes inaccessible in the Snap, open Manage Games and
choose its Restore access action. Deployd remembers the original location when the desktop portal
provides it and suggests it in the folder picker; you must select the original folder again to
authorize access. Older grants may require manual navigation and confirmation. A desktop may
ignore the suggested starting folder.

Wine-prefix recovery asks for the folder containing the original prefix. This keeps access anchored
to the containing folder when Proton or another launcher deletes and recreates the prefix itself.
Deployd reconnects only the configured prefix inside the folder you authorize.

Recovery updates games sharing that selected folder together and repairs recognized tool links and
interrupted save references. Repairs resume after restart. Modified links are preserved and reported;
affected game operations stay blocked until recovery finishes. Changing to a different installation
uses the separate Change folder action. Downloads and custom-cache recovery are unchanged, including
the manual removable-media connection needed when the external-drive Downloads portal route fails.

If a required data upgrade fails at startup, Deployd explains the problem. Recoverable
metadata problems appear as warnings and are retried on a later launch.

Deployment and purge report success only after required filesystem and tracking updates complete.
Problems cleaning empty directories or restoring optional backups appear as warnings without
hiding completed work. If moving a game cache fails partway through, Deployd attempts to move the
already-relocated files back and reports any rollback problem that still needs attention.

The header shows the selected editable profile separately from the deployed profile. Open
**Deploy options → Deployment history** to see retained storage, modification or recovery status,
restore a generation as a new profile, or delete an unprotected generation. Restoring history does
not change the game immediately. Live save banks switch only during Deploy, and saves are not part
of deployment history. External tools require the selected profile to match the deployed
configuration; after a tool exits, Deployd scans for changes without automatically sorting or
deploying them. The final Deploy confirmation summarizes file counts without listing internal
deployment targets. After this version records source identities during a deployment, later
deployments reuse the retained identities of unchanged mod files so that small library changes do
not require copying the whole library again.

**Hardlink filesystem constraint**

Hardlinks require both the cache directory and the game directory to reside on the **same filesystem**. Common examples:

| Storage setup | What counts as "same filesystem" |
|---|---|
| Standard partitions | Same partition / block device |
| BTRFS | Same **subvolume** — hardlinks cannot cross subvolume boundaries even when both folders are on the same drive |
| ZFS | Same **dataset** — hardlinks cannot cross datasets even within the same pool |
| LVM / LUKS | Same logical volume |

For games using hardlink deployment, selecting a cache directory on an incompatible
filesystem produces an explanation before any files are moved.

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
- **Archive metadata** — Manual metadata refresh uses the same Nexus mod and exact-file details as
  Mod Manager downloads. Installation uses the metadata already stored on the download and does
  not contact Nexus or request an ID. Clearing metadata restores the archive name and re-detects
  its Nexus identity when the filename contains one. Refreshed metadata remains available after restarting.
  Rescanning current Nexus filenames matches the page, file label, and version, keeping metadata
  attached to the correct archive when multiple versions of the same file are present.
- **From Nexus Mods** — Click **Mod Manager Download**; the file downloads into Deployd automatically
- **FOMOD mods** — A wizard opens automatically if the archive includes a FOMOD installer. Option
  labels remain grouped with their controls, and conditional steps support installers that use an
  empty dependency value to represent an unset flag.
- **File selection** — The Install Mod dialog lets you deselect archive entries before they are
  cached and deployed
- **Reinstall** — Use the ↺ button on any mod row to re-extract from its original archive, or use the refresh button in the Downloads panel
- **Replace** — When a name conflict occurs during install, choose Replace to swap the mod in-place,
  preserve its Mod Order position and plugin states, and make the previous archive installable again
- **Absorb external changes** — Merging a detected external file into an existing mod persists it in
  that mod. Changing the file between Data and Root removes the previous route on the next deployment
- **Rescan Cache** — Refreshes the open Mod Properties file list and preserves saved or pending
  per-file Data/Root targets for unchanged files
- **Nexus updates** — Installed Nexus files are checked together at startup; after an installation,
  only that mod is checked. Update badges follow Nexus' file-specific replacement chain, so
  unrelated optional files do not create false alerts

In the Snap package, access granted to an external downloads folder does not cover other folders
on that drive. Move a local archive into the configured downloads folder, scan it, and install it
from the Downloads panel.

### 4. Deployment

Deploy applies your enabled mods in the selected order. Most supported games use
hardlinks from the mod cache; MELE rebuilds its managed installation using separate copies.

- Toggle individual mods on/off with the switch in each row
- Enter **Select Mode** to reorder mods; in v2, Select Mode is the only way to modify load order
- Drag selected rows to reorder; mods lower in the list win file conflicts
- **Deploy** — applies your changes to the game folder
- **Purge** — removes managed mod files and restores backed-up originals

For games other than MELE, when a deployment will replace files supplied by the game, Deployd shows
the affected paths before making changes. **Protect vanilla game files** is enabled by default and
can be changed in that warning or under **Settings → Deployment**. Protected originals are verified
before deployment and restored on the next Deploy or Purge after no enabled mod uses their path.
This shared protection covers Data, game-root, engine-specific sibling, and Wine Documents targets.
Disabling protection allows an explicit unprotected deployment; originals that were already
replaced without a backup must be recovered with the game platform's file-verification feature.

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

Profiles can use shared **Global** saves or an isolated save set owned by that profile. Before
Deployd replaces live game saves, it verifies the target, captures the departing state, and keeps
an automatic recovery point. Switching back to a Global profile restores the last shared Global
state instead of promoting another profile's saves.

Use **Profile options → Manage save backups** to create named backups or inspect, restore, and
delete recovery points. Manual backups are retained until you delete them. Automatic backups use
a configurable per-game storage cap, set under **Settings → Save Backups**. Close the game before
switching save sets, changing save mode, or restoring a backup.

Cloning a profile also clones its save mode and current isolated save state. Operations that can
replace or permanently delete saves show a confirmation dialog first. Snap installations validate
Wine-prefix access before every save mutation and stop without changing files when access has
expired.

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

1. **Import** — Add mod archives to your library and choose their installation options
2. **Deploy** — Apply the enabled mods to the selected game
3. **Restore** — Remove managed mods and restore backed-up originals with Purge
4. **Profiles** — Save different mod selections and orders for each game

Deployd treats downloaded archives and installer metadata as untrusted input. Keep the application
updated so archive, XML, and network-parser security fixes are applied with new releases.

---

## Support

If Deployd saves you time, consider supporting development:

[![Support on Ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/mattianelo)

---

*Built in 🇦🇷 with [Rust](https://www.rust-lang.org) · [GTK4](https://gtk.org) · [Relm4](https://relm4.org) · [SQLite](https://sqlite.org) · and the help of agentic AI workflows*

---

GPL-3.0-only
