# Changelog

## [Unreleased]

### Added

- **Mod folders accessible to tools via M:\\** — all installed mod folders are now mapped as a Wine drive (`M:\`) when launching external tools. Tools like NPC Plugin Chooser 2 that need to see all versions of conflicting files (not just the deployed winner) can be configured to read from `M:\`. Folders are named by priority and mod name (e.g. `M:\00010-Bijin_NPCs\`) and are kept in sync automatically after deploy, purge, and priority changes.

### Fixed

- **Delta deployment now actually speeds up re-deploys** — the database was still being fully cleared and rebuilt on every deploy regardless of how few mods changed. Now only the affected files are removed or inserted, making small re-deploys proportionally faster.
- **Update notifications now work for all users** — the previous release check fell back to GitHub (where no releases exist), so free users running an older version never saw the update banner. The check now uses Nexus exclusively, matching where releases are actually published.

### Improved

- **Faster deploys** — deployd now computes a diff between the current and desired state and only touches files that actually changed. Mods that were already correctly deployed are left untouched, making re-deploys much faster on large modlists when only a few mods change.

---

## [0.9.3] — 2026-03-10 · Public Beta

### Added

- **Self-update** — when a new version is detected and deployd is running as an AppImage, the update banner shows a "Download Update" button that downloads and applies the update in-place. Nexus Premium is required; non-premium users are directed to the Nexus page instead.

### Fixed

- Switching the downloads sort order while a download is in progress no longer crashes the app
- Profile creation no longer fails when the auto-generated name already exists (e.g. after deleting and re-creating profiles)
- App now restores the last used profile on startup instead of always falling back to the first alphabetical one
- Deploy and Purge now show a warning when the game folder was last deployed from a different profile, helping avoid mixed mod state when switching profiles
- External tools (xEdit, BodySlide, etc.) now launch correctly for games added from custom directories; the Wine binary is now read from Heroic config even when a custom prefix is set, and Proton is auto-detected from Steam library and `compatibilitytools.d` as a fallback

---

## [0.9.2] — 2026-03-09 · Public Beta

### Added

- **Manage Games dialog** — accessible from Settings → Games, shows all detected games with per-game controls to change the installation path and set a custom Wine prefix. Users can also add any supported game type from a custom directory not found by auto-detection.
- **Wine prefix override** — any game (auto-detected or manually added) can now have its Wine prefix specified manually, so external tools (xEdit, BodySlide, etc.) work correctly even when Heroic config is absent or points to the wrong location.
- **First-run setup** — on first launch the Manage Games dialog is shown automatically so users can confirm detected paths and set Wine prefixes before using the app.
- **Manage Games in Settings** — the Settings panel now has a Games section with a "Manage Games" row that opens the setup dialog at any time, making it easy to reconfigure directories and Wine prefixes on existing installs.
- **Remove game from management** — a "×" button in the headerbar lets you stop managing the current game; unchecking a game in the Manage Games dialog has the same effect. Removed games are hidden from auto-detection on future launches.
- **App update notifications** — on startup, deployd checks the Nexus Mods page for a newer version (using your stored API key) and falls back to GitHub if no key is set. A banner appears in the headerbar with a direct link when an update is available.
- **Properties button** on each mod row — click the ⚙ icon to open the mod's properties window directly, without having to right-click.

### Fixed

- "Add a Game from Custom Directory" button now correctly opens the folder selection form.

---

## [0.9.1] — 2026-03-08 · Public Beta

First public release on Nexus Mods.

### Added

- **SSO login** for Nexus Mods — authenticate with one click in your browser; manual API key is now optional
- **Steam library detection** — games are now discovered from Steam alongside Heroic Launcher (GOG/Epic)
- **Fallout: New Vegas** support
- **Starfield** support
- **REDEngine** support promoted to stable — The Witcher 3 and Cyberpunk 2077 are fully supported and no longer behind an experimental toggle

### Changed

- AppImage is now the primary distribution format
- Version bumped to 0.9.1

---

## [0.9.0] — 2025-03-03 · Internal Beta

- Nexus Mods integration (API key, NXM links, update checking)
- FOMOD installer wizard with conditional steps and visibility rules
- Priority-based mod deployment via hardlinks
- Plugin load order management (`.esp` / `.esm` / `.esl`) written to `plugins.txt`
- Conflict detection with per-file detail
- Mod profiles — save and restore complete mod + plugin configurations per game
- Save management (read-only)
- External tool launcher via Wine/Proton
- Game detection from Heroic Launcher
- Experimental REDEngine support (The Witcher 3, Cyberpunk 2077)

---

## [0.1.0] — 2025-02-10

- Initial internal release
