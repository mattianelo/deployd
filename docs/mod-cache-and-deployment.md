# Mod Cache and Deployment

## Overview

deployd never copies files directly into your game folder. Instead, it maintains a **local cache** of every installed mod's files, and creates **hardlinks** from the cache into the game folder when you deploy. A hardlink points to the same data on disk as the original — no duplication, no extra space used. Removing a hardlink (purging or uninstalling a mod) never touches the cached copy, so re-deploying is instant.

This design means:

- The game folder contains only what is currently active. Disabled mods leave no trace there.
- Switching a mod on or off, changing priority, or re-deploying after installing new mods only touches the files that actually changed.
- The cache is the source of truth. As long as the cache exists, you can always re-deploy from scratch.

---

## The Cache

**Location:** `~/.local/share/deployd/cache/<mod-uuid>/`

Each installed mod gets its own UUID-named directory inside the cache root. When you install a mod from an archive (ZIP, 7z, RAR), deployd:

1. Extracts the archive to a temporary directory.
2. Applies any game-specific path transformations (e.g. Witcher 3 wraps files under `Mods/{name}/`, CP2077 routes loose archives to `archive/pc/mod/`).
3. Copies each file into the cache directory, storing it under its **lowercase-normalised path** (`Data/Textures/Foo.DDS` → `data/textures/foo.dds`). The lowercase path is used as the conflict-resolution key; the original casing is preserved separately for when the file is linked into the game.
4. Records every file in the `mod_files` database table, along with the mod it belongs to and its full cache path.

The temporary extraction directory is discarded after installation. The cache is permanent until you uninstall the mod.

---

## Deployment

Clicking **Deploy** runs the following sequence:

1. **Load current state** — read `deployed_files` from the database to know which files are currently hardlinked in the game folder and which mod each came from.

2. **Compute desired state** — for every file path across all enabled mods, pick the winner by priority. A lower priority number means higher precedence; the winning mod's cached copy is the one that should appear in the game folder.

3. **Diff** — compare current vs. desired. Files whose winning mod changed, or that are no longer needed (mod disabled, uninstalled, or outranked), are marked for removal. New winners are marked for addition.

4. **Apply** — remove stale hardlinks (and any now-empty directories). Create any new parent directories needed, then hardlink each new winner from the cache into the game folder.

5. **Update the database** — record only the affected rows in `deployed_files`. Files that were already correctly deployed are left completely untouched.

6. **Write metadata** — for Bethesda games, regenerate `plugins.txt` and the archive-invalidation INI so the game picks up the correct load order.

### Delta deployment

Only the files that actually changed between two deploys are touched on disk and in the database. If you install one new mod into a 1000-mod list, only that mod's files are hardlinked; everything else stays as-is. This keeps re-deploys fast regardless of list size.

---

## Conflict Resolution

When two or more enabled mods provide a file at the same path (compared case-insensitively), there is a conflict. The mod with the **lowest priority number** wins — its file is the one hardlinked into the game folder. Other mods' copies remain safely in the cache; they would win if the higher-priority mod were disabled.

Conflicts are detected before deployment and surfaced in the UI so you can inspect which mod is winning each file.

---

## Purge

**Purge** is the inverse of Deploy. It removes every hardlink in the game folder that deployd manages and clears the `deployed_files` table. The cache is untouched — all mod files remain on disk. Running Deploy after a Purge restores the full deployment from the cache.

Purge is useful when you want to verify the game works without any mods, or before uninstalling deployd entirely.
