# Changelog

## [Unreleased]

- Fixed manual Nexus metadata refreshes for archived downloads so nullable API fields no longer
  fail parsing, exact file names and versions update through the same result path as NXM downloads,
  file-ID prompts wait for filename matching to finish, and completed or failed refreshes no longer
  leave a download marked busy. Current Nexus archive filenames now yield the correct mod ID rather
  than a number from their timestamp, and clearing metadata rebuilds that identity from the archive.
  Installing an archive now uses only stored metadata and never performs an automatic Nexus refresh.
- Strict-Snap manual installs from an inaccessible folder on the same external drive as the
  configured downloads folder now show recovery instructions instead of a raw OS error.
- Reworked profile save management around verified Global and per-profile save banks. Profile
  switches now capture unsynced live saves, create versioned recovery points, restore through a
  rollback-capable transaction, and recover interrupted transitions without treating a missing
  snapshot as an empty save directory.
- Added named save backups with restore and delete controls, SHA-256 integrity manifests, a
  configurable 5 GiB-per-game automatic-backup cap, and confirmation dialogs for save-mode,
  synchronization, restore, profile deletion, and backup deletion operations.
- Cloned profiles now inherit their source save mode and current isolated saves, returning to a
  Global profile restores the shared Global state, and strict-Snap builds revalidate Wine-prefix
  access before every save mutation.
- Fixed new Snap installations being unable to select an external drive as the downloads folder by
  requesting persistent access through the desktop portal, showing validation feedback in the
  Downloads section, and showing a copyable manual removable-media connection prompt for direct
  external paths or inaccessible external-drive portal routes. The prompt now targets the active
  parallel Snap instance, and application-scoped document-portal paths resolve without duplicating
  the portal's synthetic document basename.
- Development analysis now runs compiler-backed semantic and structural checks through the
  project's isolated, reproducible environment.
- Pinned the Rust development toolchain and isolated local AppImage checks and builds under an
  explicit non-root LXD user, with Cargo output kept outside the bind-mounted repository.
- Hardened the project check wrapper with an explicit command allow list, protected feature and
  target configuration, environment diagnostics, and shell-level policy tests.
- Added an opt-in, single-package lockfile maintenance command so approved security updates can run
  through the same non-root LXD boundary without enabling arbitrary Cargo commands.
- Moved root-required Snapcraft builds into container-owned scratch storage and limited host
  export to the completed Snap artifact.
- Confined GitLab Pages generation to the checkout's non-symlinked `out/` directory, preventing
  an environment override from selecting an arbitrary recursive-deletion target.
- Updated XML, concurrency, random-number, QUIC, and TLS certificate dependencies to RustSec-patched
  releases, restricted SQLx to the SQLite feature set used by Deployd, and made auditing reject the
  reviewed inactive-driver exception if RSA ever becomes reachable.
- Recoverable startup, installation, and stale-dialog failures now report or safely cancel the
  affected action instead of terminating Deployd.
- Database startup now stops with actionable context when a required schema upgrade cannot be
  applied, while retryable metadata backfills surface warnings and are retried on a later launch.
- Deployment and purge now stop on required filesystem or tracking failures instead of reporting
  a successful synchronization. Recoverable cleanup and backup-restoration problems are surfaced
  as warnings, and partial cache relocations are rolled back when possible.
- Stable version tags now validate all release metadata before GitLab publishes the AppImage to
  Nexus Mods and the GitHub mirror independently builds and publishes the Snap to the stable
  Snap Store channel; manual GitHub runs build an auditable Snap artifact without store access.
- Hosted core24 Snap builds now provision the pinned Rust toolchain through Snapcraft's recognized
  `rust-deps` part and Ubuntu's package-compatible default-toolchain flow, avoiding clean-runner
  validation failures and the incompatible core26-based Rustup build snap.
- GitLab now creates pipelines only for explicit manual preflights, scheduled maintenance, and
  exact stable release tags; ordinary commits and merge-request updates no longer consume runner
  minutes, and Pages publication is release-driven or manually requested.

## [2.3.3]

- Game and profile restoration now ignores stale asynchronous load results, preventing a slower
  load for another game from replacing the currently selected game's profiles, mods, and plugins.
- AppImage startup now repairs missing or stale NXM protocol integration even when a restored data
  folder contains an old registration marker.
- Snap game loads now detect expired document-portal grants, explain that the installation folder
  or Wine prefix must be reselected, and suppress misleading missing-dependency warnings when the
  game `Data` directory could not be completely scanned.
- Added an isolated Ubuntu 24.04 LXD workflow for local Snap builds, using the GNOME SDK for its
  GTK dependencies and avoiding Snapcraft 9's incompatible core26-based Rustup build snap.

## [2.3.2]

- Witcher 2 save management now uses `Documents/Witcher 2/gamesaves`, so profile backups find
  the game's actual save directory instead of creating an unused `The Witcher 2` folder.
- Snap builds now explicitly target amd64, pin Rust 1.96.0, enforce the Cargo lockfile, restore
  library linting, and publish valid AppStream screenshot and license metadata for automatic builds.
- AppImage-to-Snap migration now preserves installed-mod source metadata and the valid profile
  associated with deployed files, keeping download and deployment state consistent after import.

## [2.3.1]

- Games now persist the active profile as part of successful deployment completion and reopen it
  independently across restarts and game switches. Stale GTK dropdown events generated while
  rebuilding the profile list can no longer switch back to the alphabetically first profile.
- Routine status messages, including external-tool exits, now appear as temporary toasts instead
  of accumulating in the notifications panel; failures and actionable recovery notices remain.
- Normal external-tool exits now scan for changes, run LOOT where supported, and automatically
  deploy the resulting order. Cancelled sessions skip the workflow; games without LOOT support
  deploy directly.
- The Downloads panel now uses one dedicated sort menu button with direct order choices instead of
  nesting a dropdown beneath the generic three-dot overflow button.
- Stable version-tag pipelines now publish the AppImage to Deployd's existing
  Nexus Mods file through a repository-owned client of the Nexus v3 upload API.
  CI packages the executable AppImage in the ZIP format accepted by Nexus before
  using a multipart upload session. Neither step has a GitHub Actions runtime
  dependency. The API key and Nexus file ID remain protected GitLab CI/CD
  variables, and manual branch pipelines cannot publish.
- Pre-install re-scans now keep skipping FOMOD metadata, so deselected or hidden `fomod/`
  folders no longer cache stray installer XML files.
- Fixed Snap download archive trashing when the stored archive path points at an XDG
  document-portal mount.
- Fixed Snap folder validation rejecting persistent XDG document-portal grants, which prevented
  selecting downloads and other folders on external drives.
- Snap folder selections are now validated before they are saved for game folders,
  Wine prefixes, downloads folders, cache folders, and AppImage migration imports. Deployd now
  explains blocked hidden-home, ungranted removable-media, and read/write access failures at
  selection time instead of surfacing them later as confusing deploy or scan errors.
- AppImage-to-Snap migration import no longer asks the Snap to confirm or save a
  downloads folder. Imported download rows keep fetched Nexus metadata, hashes, versions,
  and authors, while stale AppImage archive paths are still cleared for later rescans.
- Opening update pages, deployment folders, and mod cache folders now reports a notification or
  dialog when the desktop cannot open the target, and Snap builds use the update notification's
  "View" action to open the release page instead of invoking AppImage self-update logic.
- Added a GitLab Pages feature page with screenshots, animated feature highlights,
  and download links for the Snap Store and Nexus Mods. The Pages job generates
  and publishes an `out/` directory, and the local preview can run through a
  dedicated LXD container.
- Updated documentation to reflect that Deployd is now released on the Snap Store,
  including install instructions and accepted Snap Store review status.
- Snap CI now pulls the official core24 Snapcraft image from GitHub Container
  Registry and clears its default entrypoint for GitLab Runner before invoking
  `snapcraft pack --destructive-mode`.
- Manual Nexus metadata fetches now keep the selected or inferred mod page authoritative when
  checking archive MD5s, so files like `NEO-65761-3-1-1-1763043682` resolve against the Fallout 4
  mod page `65761` instead of adopting unrelated file metadata.

## [2.3.0]

### Added

- AppImage-to-Snap migration export creates a per-game `.deployd-export.zip` bundle from Manage
  Games with that game's database slice, cached mod files, vanilla backups, and profile save
  snapshots. The migration UI is available only when experimental features are enabled.
- Snap builds can preview AppImage migration bundles from Settings without importing them, showing
  the exported game, content counts, warnings, existing-game conflicts, and required later path/tool
  confirmations. The migration UI is available only when experimental features are enabled.
- Snap builds can now import a previewed AppImage migration bundle for a new game after confirming
  Snap-visible game, Wine prefix, and downloads folders. Import copies cache, vanilla backups, and
  save snapshots into Snap-owned storage, skips AppImage external tools, clears stale download
  archive paths, and refuses existing-game bundles without overwriting Snap state. The migration UI
  is available only when experimental features are enabled.
- Performance feedback is more cohesive during long-running work: install, extraction, caching,
  deploy, purge, downloads-folder scan, and first-run runtime setup now share the same header busy
  status language.
- Downloads-folder scanning now runs off the GTK thread, and install progress updates are throttled
  so large archives no longer flood the UI with per-file updates.
- Downloads rows mirror install phase status for archive hashing, extraction, setup preparation,
  caching, and Nexus metadata fetches, so installs started from the Downloads panel remain visible
  even when the header focus changes.
- Debug builds now log lightweight slow-phase timing for install, scan, deploy, plugin-header, and
  tool-launch preparation paths without recording user paths or API tokens.
- Mod Properties now lets users correct the Nexus Mod ID for an installed mod when filename or
  download metadata detection picked the wrong Nexus page.

### Fixed

- Plugin Order grouped drag now uses Deployd's app-owned Select Mode state, so dragging a selected
  plugin moves the whole selected block and keeps that block selected after the drop.
- Plugin Order stays in Select Mode after grouped drops and preserves the scroll position instead
  of jumping back to the top of the list.
- Fetching metadata for an older version of an already-installed mod no longer overwrites the
  installed mod's version. The Downloads panel previously wrote the resolved version to the
  mods table keyed on mod id alone, so a later Mod Order reload would surface the older version
  in place of the actually installed one. The version write now flows exclusively through the
  file-id-aware path that matches on `(game_id, nexus_mod_id, nexus_file_id)`.
- Snap now claims the `io.mattianelo.deployd` session D-Bus name instead
  of `app.deployd`, and its desktop/AppStream IDs match that reverse-DNS
  identity for manual Snap Store review.
- Search typing is debounced to reduce list-filter stalls while keeping filter chips immediate.
- Downloads rows no longer stay stuck in a busy state when a download completes during another
  install, or when Nexus metadata fetching succeeds or fails after temporarily showing row-level
  progress.
- Tool launches no longer try to reuse a Wine drive letter occupied by a broken `dosdevices`
  symlink, avoiding noisy `failed to create q: drive: File exists` messages after BodySlide runs.
- BodySlide detection and launch now prefer `BodySlide x64.exe` over the 32-bit `BodySlide.exe`,
  avoiding 32-bit GLX failures in UMU/pressure-vessel when opening Preview.
- Tool launch buttons no longer remain stuck disabled after being rebuilt during a busy state, and
  normal launches now show a blocking launch dialog with spinner, status text, and Cancel while
  Wine/UMU starts the tool.
- External Tools launches are tracked as managed sessions; Cancel now stops pre-spawn setup and can
  terminate the Deployd-owned Wine/UMU process group after the tool has spawned.
- Vanilla/DLC plugin header master counts are cached by game, path, mtime, and file length to avoid
  repeated TES4 header reads during ordinary reloads.
- Snap copy-fallback deployments no longer make managed plugins appear as externally cleaned when
  the deployed file still matches Deployd's cache.
- Downloads-folder scans now preserve and reattach imported AppImage download metadata, including
  downloaded-but-not-installed entries, instead of creating duplicate path-only rows after a Snap
  migration import.
- Downloads-folder scans now reconcile duplicate archive rows only by strong archive/file identity,
  such as exact archive paths, exact Nexus file IDs, or exact normalized Nexus filenames. Multiple
  archives from the same Nexus mod page are preserved as separate downloads.
- Installed mods now retain source metadata independently from the downloads inventory, including
  Nexus file names, primary-file flags, and archive MD5 values backfilled from installed download
  rows.
- Downloads-folder scans now hide pathless metadata cache rows instead of deleting them, so changing
  downloads folders can clean visible archive inventory without forcing a full Nexus metadata refetch.
- Snap migration import now treats previously stopped/hidden games as re-importable, replacing
  their hidden Snap state, while still refusing games that are actively managed.

## [2.2.1]

### Fixed

- Mod Order header spacing now matches the sibling Plugin Order and Downloads panels.
- Downloads context-menu delete action remains visible in AppImage light mode.
- Mod Properties now prioritizes per-file target and conflict management, gives large file lists
  more working room, and moves secondary details lower in the dialog.
- Install Mod and Mod Properties no longer show redundant static section titles above their
  collapsible file/conflict controls.
- Install Mod now honors deselected files before engine-specific path routing, fixing Witcher 1
  installs where skipped archive entries could still be cached and deployed.

## [2.2.0]

### Added

- **Multi-row selection in Plugin Order panel** — Ctrl/Shift click to build a selection; drag the
  block to move all selected plugins together in one operation.
- **Color-coded mod groups** — group headers display a tinted dot; a color palette (clear + 8
  swatches) appears in the rename popover to set or clear the color.
- **LOOT sort notifications as toasts** — LOOT sort results are delivered as toasts rather than
  accumulating silently in the notification panel.
- **Conflict dot icon with tint** — the per-row conflict indicator is a single dot icon; yellow
  when the mod is overridden, green when it only overrides. Tooltip combines both directions.
  A `· N conflicts` label is added to the status bar, hidden when zero.
- **Filter empty states, enabled-mod tint, UI polish** — mod list shows an empty-state placeholder
  when filters return no results; enabled mods receive a subtle background tint; archive paths use
  monospace; group headers highlight on hover; dropdowns use the flat style.
- **Downloads panel improvements** — delete individual download entries, hide the panel entirely,
  resizable sidebar (250 – 700 px) via `adw::OverlaySplitView` replacing the legacy `gtk::Paned`.
- **Notification improvements** — clear-all button in the notification panel header; expandable
  rows for long messages; scroll position preserved after delete.

### Fixed

- External tool runtime split is now package-specific: AppImage launches through bundled UMU with
  Proton GE stored under Deployd's data directory, while Snap launches only through its Wine content
  interfaces and shows plug-connection guidance when they are unavailable.
- First-run AppImage Proton GE setup now shows a busy status until UMU finishes preparing the
  Deployd-managed runtime. The Snap Wine prompt now shows the missing interface command with a
  copy button so users can run it in a terminal before launching again.
- Plain click replaces the current selection; Ctrl/Shift still extend it.
- Pressing Escape or clicking an empty area clears the list selection.
- Version-column migration guard prevents a duplicate-column error on upgrade.
- Selection tint refreshes correctly after a tracker reload.
- Unselect is deferred past the drag-drop commit so drops land in the right position.
- File and conflict list rows use raw `ListBoxRow` with 5 px margins, replacing
  `adw::ActionRow` whose 52 px Adwaita height made rows appear too tall.
- Window no longer flashes black on first show; `present()` is deferred to the next idle tick.
- Dependency load-order panel drag-and-drop was blocked by a stale sensitivity guard.
- Collapsed group drag now moves the separator and all hidden member mods as a block.
  Expanded group drag moves only the separator (members follow by position).
- Downloads: version field is always propagated from a metadata fetch.
- Deployer: vanilla backup is only created for files that existed in the pre-mod baseline;
  files placed by other tools are not backed up and will not be unexpectedly restored.

### Changed

- **Complete libadwaita migration** — the primary app shell, dialogs, popovers, transient states,
  and factory-backed main lists now follow modern GNOME HIG patterns while preserving existing
  Relm4 messages, drag/drop behaviour, Snap/AppImage paths, and mod-management logic.
- **Main window rebuilt around adaptive libadwaita structure** — the root app shell now uses
  `adw::ToolbarView`, the Mod Order and Plugin Order panes use `adw::NavigationSplitView`, pane
  headers use local `adw::HeaderBar` controls, and downloads remain in an `adw::OverlaySplitView`
  with the existing sidebar width behaviour.
- **Popover and transient UI converted to GNOME rows and alerts** — profile, deploy, overflow,
  snapshot, notification, and Nexus account popovers now use boxed lists, preferences groups, and
  `adw::ActionRow` patterns where practical; app workflow prompts now use `adw::AlertDialog`.
- **Dialog workflows migrated to libadwaita** — FOMOD, Mod Properties, Pre-install, Absorb External
  Changes, Tool Manager, Game Setup, and Welcome Wizard now use libadwaita toolbar, clamp,
  preferences, action-row, status, and action-bar patterns. The Absorb dialog also opens at a larger
  default size.
- **Main list rows migrated to libadwaita patterns** — mod, plugin, and download rows now use
  `adw::ActionRow`-style structure for a more consistent GNOME HIG presentation while preserving
  existing drag/drop, selection, metadata, and row action behaviour.

### Removed

- **Experimental features deleted** — Profile Import/Export, Script Extender Launch, and
  Backup & Restore were fully implemented but not production-ready. The implementations,
  their UI surfaces, messages, and backing tracker functions have been removed entirely.
- **Compact row settings removed** — mod and plugin lists now use one balanced row density rather
  than separate compact/non-compact modes.

### Packaging

- Snap build now passes `--features loot,libarchive-fallback` (parity with AppImage).
- `libunrar-dev` added to Snap build-packages (parity with AppImage Dockerfile).
- `python3` removed from Snap stage-packages; it was only staged for UMU Launcher,
  which cannot run inside a Strict snap (bwrap/pressure-vessel blocked).
- Snap CI job re-enabled; triggers on version tags and manual pipeline runs,
  matching AppImage CI behaviour.

## [1.1.1]

### Added

- **Mod author propagated to Mod Order panel** — installing a mod whose metadata was fetched via
  the Downloads panel (manual fetch or right-click) now correctly carries the author name through
  to the Mod Order row. Previously the author was discarded after the metadata fetch and never
  reached the panel.
- **Scroll position restored after mod list reload** — the Mod Order panel now preserves the
  vertical scroll position across reloads, so a reload triggered by install or re-scan no longer
  jumps the view back to the top.
- **Drag dead zone** — a small movement threshold must be exceeded before a mod-row drag is
  initiated, preventing accidental reorders from stray click motion.
- **Version metadata displayed in Downloads panel** — the Nexus file version is fetched alongside
  mod name and author, and shown in the Downloads row so multiple versions of the same file are
  distinguishable without opening Properties.

### Changed

- **Toast notifications** — download progress, deploy results, and install confirmations are now
  delivered as libadwaita toasts instead of the notification bubble, giving faster dismissal and
  better integration with the app window.
- **UI migrated to libadwaita 1.5** — all widgets updated to use the latest libadwaita patterns
  (AdwNavigationView, AdwToolbarView, AdwActionRow, AdwBanner); deprecated shims removed.

### Fixed

- **Empty notification bubble** — the notification indicator no longer shows a bubble when the
  in-progress queue is empty.
- **Deploy toast shown after install** — the post-install deploy toast was silently suppressed in
  some paths; it now appears consistently.
- **Metadata version race on install** — a race between the metadata-fetch completion and the
  install flow could leave the version field unpopulated; the fetch result is now applied before
  the install proceeds.
- **In-progress download/metadata notifications rerouted to toasts** — mid-operation status
  messages were incorrectly going to the notification bubble instead of toasts.

---

## [1.0.1]

### Added

- **Mod name autocomplete in Install dialog** — the Mod Name entry in the Install Mod dialog now
  offers autocomplete suggestions from existing mod names (triggers after 2 characters), making it
  easier to merge an install into an existing mod slot.
- **Compact Mod List toggle** — Settings now includes a "Compact Mod List" switch (alongside the
  existing plugin toggle) to reduce row height in the Mod Order panel. The setting persists across
  restarts.
- **Drag-scroll in Mod List** — dragging a mod near the top or bottom edge of the list now
  auto-scrolls the panel, making it practical to reorder mods across long lists without dropping.
- **Stable `named_mods/by-name/` symlink directory** — the named mods folder now also populates a
  `by-name/<name>` subtree with prefix-free symlinks. These paths never change when mods are
  reordered, providing a stable output target for tools like PGPatcher.
- **Archive path stored; reinstall from mod row** — the source archive path is recorded when a mod
  is installed. A reinstall button (↺) appears on each mod row when the archive is still present on
  disk, re-opening the install flow for that archive. The archive filename is also shown in the mod
  Properties dialog.
- **Conflict detail in Mod Properties** — the Properties dialog now shows a collapsible Conflicts
  section listing which files this mod overrides (wins), which of its files are overridden (loses),
  and which mods are involved on each side.

### Fixed

- **Cancel on "External File Changed" dialog restores notification** — cancelling the absorb
  dialog no longer loses the pending file list. The notification reappears so the user can retry.
- **Group drag picks up extra mods on second drag** — dropping a group separator now snaps to the
  nearest valid group boundary, preventing it from landing mid-group and silently absorbing the
  mods between it and the next separator on the following drag.
- **Pre-install dialog warns when files are auto-assigned to game root** — a banner appears when
  any file is auto-detected as Root (exe/dll/asi), prompting the user to switch to "Set all → D"
  for tools like Pandora whose data files must share the same folder as the executable.

---

## [1.0.0]

### Added

- **Per-file deselection in Install Dialog** — each file row in the Install Mod dialog now has a
  checkbox. Unchecked files are excluded from the install; they are not copied to the mod cache
  and not registered in the database.
- **Install Dialog file list now taller and resizable** — the file list now shows up to 500 px of
  content by default and the window can be resized freely.
- **Install Dialog shows files for all engines** — the file list (with deselection checkboxes) is
  now shown for REDEngine and Eclipse mods in addition to Bethesda and Aurora. Data/Root toggle
  buttons still only appear for Bethesda and Aurora.
- **FOMOD: persistent selections and hover preview** — the FOMOD installer now remembers your
  selections when navigating between steps, and hovering an option shows its image inline without
  clicking.

### Added (experimental)

- **Script extender launcher for Steam** — games with a script extender (SKSE, F4SE, etc.) can
  now be launched directly from Deployd via Steam, with `SteamAppId` and `SteamGameId` set
  correctly so the extender attaches.
- **Backup and restore** — full mod-cache backups can be created and restored from Settings,
  covering the database and all cached mod files. Useful before large migrations.

### Changed

- **Install dialog: file list expanded by default** — the file list opens fully expanded; the
  global Install To toggle has been removed (per-file targets are set directly in the list).
- **Rescan Cache renamed** — the "Scan Cache" button is now labelled "Rescan Cache".

### Fixed

- **Mod Properties Root/Data order** — opening the Properties dialog for a mod whose files were
  installed to the game root now correctly shows "Root" in the global Install To toggle. Previously
  the toggle was initialised from the `install_target` column, which is set to Data for mixed-target
  mods; the dialog now derives the toggle state from the actual per-file targets loaded from the
  database, so clicking Apply no longer resets Root files to Data.
- **Downloads: MD5-based file matching** — when Nexus filename matching is ambiguous, the archive
  MD5 is computed and used to identify the exact file entry, with `uploaded_timestamp` as a
  tiebreaker. The file-ID entry dialog is shown as a fallback when all automatic paths fail.
- **Downloads: version shown in panel** — the file version is now stored and displayed in the
  downloads panel so multiple versions of the same mod are distinguishable at a glance.
- **Downloads: right-click metadata fetch shows file-ID dialog** — fetching metadata from the
  context menu now opens the file-ID dialog when filename matching fails, instead of silently
  doing nothing. The context menu popover closes immediately after selection.
- **Conflict detection: directory sentinels excluded** — two mods sharing only an empty folder
  (path ending with `/`) are no longer reported as conflicting.
- **Plugin compact mode persisted on restart** — the compact plugin list setting now survives
  app restarts.
- **NXM link and timestamp parsing hardened** — malformed NXM URLs and unexpected timestamp
  formats no longer cause a panic.
- **FOMOD: duplicate `flagDependency` handling** — malformed FOMOD configs that declare the same
  condition flag more than once no longer cause incorrect step visibility.
- **GTK theme-parser warnings suppressed** — remaining benign CSS warnings from the GTK theme
  parser are filtered at runtime and no longer appear in the log.

### Fixed (experimental)

- **Snap: Mono install allowed for Eclipse tools** — the AppArmor profile now permits the Mono
  installer to run, fixing first-time tool setup for Eclipse (Dragon Age: Origins) mods.
- **Snap: LD_PRELOAD cleared before Wine, DAZIP scan fixed, Mono dialog suppressed** — Wine no
  longer inherits the Snap-injected `LD_PRELOAD`, DAZIP archives are found correctly on rescan,
  and the Mono installation dialog is suppressed during Wine prefix initialisation.

### Build

- **Experimental feature flag** — `--experimental` build option (and `DEPLOYD_EXPERIMENTAL=1`
  in `check.sh`) gates the Launch button, profile export/import, and Backup & Restore UI so
  stable builds stay clean.
- **Pure LXD local builds; CI registry image** — local builds now use a direct LXD container
  instead of the LXD → Docker chain. CI publishes a `deployd-build-env` image to the GitLab
  container registry on Dockerfile changes, used by the `build-appimage` job.

---

## [0.9.9]

### Added

**Aurora: filename-based conflict detection for Override/ files** — The Witcher 1
  resolves Override/ files by filename alone, regardless of subfolder depth. Two mods
  providing `override/ModA/items.xml` and `override/items.xml` are now treated as
  conflicting. A new `conflict_key()` hook on `EngineHandler` lets Aurora return just
  the filename; `compute_overrides` and `compute_winners` group by this key so the
  lower-priority mod's file is excluded from deployment rather than letting filesystem
  ordering decide.
- **Portal folder pickers** — folder selection dialogs in the game-setup and welcome
  wizard flows now use the XDG Desktop Portal (via `ashpd 0.13`), making them work
  reliably on immutable distros (Bazzite, etc.) that require portal for file-dialog
  access.
- **The Witcher 2: Assassins of Kings support** — REDEngine game (GOG and Steam). Mod files deploy under `CookedPC/` inside the game directory.
- **Per-game configurable cache folder** — relocate a game's mod cache to any directory via Settings → Manage Games. The chosen path is validated against the game folder's device before moving anything; a clear error is shown if hardlinks would cross a BTRFS subvolume or ZFS dataset boundary. Cache moves use `rename(2)` when on the same device and fall back to recursive copy + delete otherwise.
- **Nexus avatar button in headerbar** — when logged in, a circular avatar button shows your Nexus profile picture (with initials fallback). Clicking it opens a popover with Login / Logout actions, replacing the Settings-panel login flow.

### Changed

- **Settings panel hides login controls when authenticated** — the Nexus Mods SSO section (and manual API key entry) is no longer shown in Settings when you are already logged in. Login and logout live exclusively in the headerbar avatar popover.
- **"Create mod group" button moved to Mod Order panel** — the button now sits in the Mod Order toolbar alongside "Add mod from file", where it conceptually belongs, rather than in the main headerbar.

### Fixed

- **Aurora: four conflict detection fixes** —
  - Mods no longer conflict with themselves when two of their Override/ files share a
    filename under different subfolders (deduplication by `mod_id` in
    `compute_overrides`)
  - Readme, licence, and changelog files are excluded from conflict reporting via a new
    `EngineHandler::is_conflict_key_ignored` default method
  - Conflict icons reload immediately after drag-and-drop reorder (calls
    `reload_mods` from `handle_cmd_priority_saved`)
  - Conflict tooltips now name the conflicting mods alongside file paths (e.g.
    "Overrides 2 file(s) — ModA, ModB")
- **Aurora: four Witcher 1 deployment bugs** —
  - Install Mod dialog legend corrected: D = Data/ (not just Override); R includes
    Register
  - Scan Cache strips the `data_subdir` prefix and marks system/launcher/register
    paths as Root (`../`), preventing `Data/data` double-nesting
  - `ensure_dirs_case_insensitive` resolves filenames case-insensitively so
    `Scripts/Mod.lua` correctly replaces `Scripts/mod.lua` on Linux
  - Mod Properties dialog now shows Data / Root toggles for Aurora mods, enabling
    post-install target changes
- **Download name update after post-install metadata fetch** — threads `download_id`
  through `NexusMetadataFetched` so the resolved Nexus mod name is written back to
  the downloads panel entry when metadata was still unresolved at install time
- **Group collapse state preserved** — mod groups no longer unexpectedly expand when installing a new mod, reordering, or creating a group. In-session collapse state is retained across all list reloads.
- **Compact Plugin List and Color Scheme settings persisted** — both appearance preferences are now saved to the database and restored on startup; they no longer reset to defaults after restarting the app.
- **Vanilla plugins sorted by dependency depth** — root masters (e.g. `Fallout4.esm`, `Skyrim.esm`) now appear first in the vanilla section, sorted by (tier, master count, name) instead of alphabetically last.
- **Cleaned vanilla plugins shown as locked** — a managed plugin that replaces a vanilla game file (detected via `vanilla_backups`) is displayed in the vanilla section as "Vanilla / Modified" with a read-only checkbox and no drag handle, rather than as a freely-movable managed plugin.
- **`nexus_file_id` written back after manual metadata fetch** — the file ID resolved during a manual metadata fetch is now persisted to the download entry so subsequent installs carry the correct ID.
- **Mod name proposed from Nexus when metadata unresolved** — when installing a download that has Nexus IDs but no fetched metadata, the mod name is fetched from Nexus in the background and pre-filled in the pre-install dialog instead of showing the raw archive filename.
- **Nexus WebP avatars display correctly in AppImage** — the avatar fetch now sends a `User-Agent` header and the AppImage bundles `webp-pixbuf-loader`, so Nexus profile pictures in WebP format decode and render properly.

### Internal

- **EngineHandler trait** — replaces scattered `if game.engine == GameEngine::X`
  chains in `deployer.rs` and `installer/mod.rs` with a trait implemented by four
  zero-sized unit structs (`BethesdaHandler`, `REDEngineHandler`, `EclipseHandler`,
  `AuroraHandler`). Each engine owns its deployment logic in a dedicated module.

### Build

- **AppImage: LXD-wrapped Docker build** — mirrors the snap workflow; the build
  script launches an Ubuntu 24.04 LXD container with `security.nesting=true`, mounts
  the repo via an idmapped disk device, installs Docker inside, then re-invokes the
  script with `DEPLOYD_NO_LXD=1`. Falls back to direct Docker when LXC is
  unavailable. The `DEPLOYD_NO_DOCKER=1` CI path is unaffected.

---

## [0.9.8]

### Added

- **Snap package** — Deployd is now available as a Snap with strict confinement. Supports all features including external tools via the bundled Wine runtime.
- **The Witcher 1 support** — Aurora engine (The Witcher 1, GOG and Steam) games can now be added and managed, including save path support.
- **First-run Proton GE wizard** — When launching an external tool for the first time with no Proton runtime present, a dialog prompts before Deployd downloads Proton GE (~300 MB) from GitHub. The tool launches automatically once the download finishes.
- **Download pause/resume** — active downloads can now be paused and resumed mid-transfer.
- **Compact mode and color scheme picker** — new Appearance settings let you switch color scheme and enable compact rows for both the mod list and plugin list.
- **Notification bell with badge** — the notification indicator is now a bell icon with a count badge.
- **Remove game confirmation dialog** — removing a game now asks for confirmation and offers an option to also delete all of its managed mods.
- **LOOT sort triggers deploy** — running LOOT sort now sets the deploy-needed flag and shows a toast so the change is not silently lost.

### Changed

- **External tool launching refactored** — tools now invoke Proton GE's `wine` binary (`files/bin-wow64/wine`) directly, bypassing pressure-vessel/bwrap entirely. Proton GE is downloaded from GitHub releases on first use. The Snap package uses `wine-platform-runtime-core22` and `wine-platform` content plugs in place of the previous bwrap-based approach.
- **UI overhaul** — filter chips on the mod list, a persistent status bar, split deploy menu button, and redesigned badges throughout.

### Fixed

- **Eclipse engine Override deployment preserves subfolders** — the `strip_eclipse_override_wrappers` pass that was flattening unrecognised path components has been removed; subfolder structure is now retained correctly.
- **Snap Wine interface connection command** — the Snap plug now matches the available `wine-platform:wine-base-stable` provider, and the setup dialog includes explicit provider slots so `snap connect` does not fall back to the system `snapd` content slot.
- **Wine DLL settings persist across Snap/Wine updates** — DLL overrides are now baked into the Wine prefix registry during setup so Snap or Wine updates cannot re-trigger the Mono/WineCfg dialogs.
- **AppImage detects WoW64 Proton GE** — the Proton GE detection path now handles the WoW64 layout introduced after the Snap packaging changes.
- **Aurora engine System/ routing corrected** — external files under `System/` are now detected and routed to the correct deployment path.
- **Group drag-and-drop moves whole blocks** — dragging a group header now moves all rows in the group together instead of only the header row.
- **Deploy popover dismisses correctly** — the deploy menu button popover now closes as expected after selecting an action.
- **Deploy button corner styling** — the split deploy menu button no longer renders with double-rounded corners.

---

## [0.9.7]

### Fixed

- **AppImage icons and theming now match the host desktop** — the host icon theme (e.g. Yaru-purple) and GTK theme are now applied correctly instead of falling back to bare Adwaita. Icons that previously rendered blank (window chrome, toolbar, Ko-Fi button) are now displayed properly.

### Changed

- **AppImage packaging overhauled** — replaced the manual `ldd`/regex-based library bundling script with a Docker-based pipeline using `linuxdeploy` and `linuxdeploy-plugin-gtk`. The build environment is fixed to Ubuntu 24.04 LTS, providing a stable glibc/ABI floor and eliminating the forward-compatibility breakage seen when building on newer host distributions.
- **AppImage is now self-contained** — the GDK pixbuf loaders cache is regenerated during packaging to reference the AppImage's own bundled loaders (including the SVG loader), rather than the build host's system paths.

---

## [0.9.6]

### Added

- **Experimental Dragon Age: Origins support** — Dragon Age: Origins (Steam and GOG) can now be added and managed. Override mods and `.dazip` archives are both supported; the game type is marked "(Experimental)" in the dropdown to reflect early-stage support.
- **FOMOD image previews** — the FOMOD installer now displays each option's image above the selection list. The preview updates as you change your selection.
- **Mod notes** — mods can now have personal notes written in the Properties dialog. Any mod with notes shows a small icon in the list; hovering it previews the note.
- **"Replace" option in name-conflict dialog** — when installing a mod whose name already exists, you can now choose Replace in addition to Merge and Create New. Replace swaps the existing mod in-place, preserving its load-order position and plugin states.
- **Reinstall button in Downloads panel** — installed downloads now show a refresh button. Clicking it re-extracts the archive and opens the pre-install dialog ready to replace the existing mod, skipping the "already installed" prompt.
- **Save / Load order snapshots** — both the Mod Order and Plugin Order panels now have Save and Load buttons alongside All / None. Save stores the current order under a custom name; Load lets you browse and restore any saved snapshot, or delete ones you no longer need. Snapshots are per-game and independent of profiles.

### Changed

- **Notification badge shows item count, not file count** — the bell badge now shows the number of distinct notification types (e.g. "1" for external changes, regardless of how many files changed). The file count is still shown in the notification row's subtitle.
- **App update notification moved to the notification popover** — the update banner has been replaced by a row in the notification popover. The button reads "Download" when running as AppImage, or "View" otherwise.

### Fixed

- **Drag-and-drop placement is now precise** — when dragging a mod or plugin, dropping it on the top half of a row places it *before* that row and dropping on the bottom half places it *after*, matching the visual indicator line. Previously items could land one position off from where the line suggested.
- **Mod and plugin lists no longer flicker or reset after reordering** — moving mods or plugins via drag-and-drop now updates the priority labels (#1, #2, …) in-place without triggering a full list rebuild. Scroll position and visual state are preserved.
- **Purge now correctly targets the active game's files** — the deployed-files tracker now stores which game each file belongs to. Previously purging could silently do nothing if files from a different game were in the tracker (e.g., after an app update or a game switch). When nothing is tracked a clearer message is shown instead of a silent no-op.
- **Dragon Age: Origins must now be explicitly enabled** — DAO support is off by default. Enable it under Settings → Games → "Dragon Age: Origins (Experimental)", then rescan for games. This prevents the game from appearing unexpectedly for users who haven't opted in.
- **CharGenMorph Compiler installs and auto-detects correctly** — tool executables (`.exe`, `.dll`, `.bat`) for Dragon Age: Origins are now deployed to the Wine user's Documents folder instead of the game's override directory. After deploying the mod, Manage Tools will automatically find the executable.
- **Search filter preserved after removing a mod** — deleting a mod no longer resets the search bar, active filters remain applied after the list refreshes.
- **Downloads rescan removes deleted archives** — when rescanning the downloads folder, entries whose archive files have been deleted are now removed from the database instead of accumulating as invisible zombies.
- **Installation progress no longer shifts the mod list** — the spinner and progress bar now occupy a fixed-height area that doesn't change size, so the mod and plugin lists stay in place during extraction and caching.
- **DAO: standalone `.dazip` files install to the correct location** — selecting a `.dazip` file directly in the install dialog now places content in `AddIns/<UID>/` rather than `packages/core/override/`.
- **DAZIP mods now appear in the Dragon Age: Origins in-game modlist** — deployd now writes to `Settings/AddIns.xml` (matching the game's exact filename). Previously it wrote `Addins.xml` (different case), which is a separate file on Linux and never read by the game. `<AddInItem>` elements are now correctly captured from manifests, and pre-existing game entries (campaigns, DLCs) are preserved on every deploy.
- **DAZIP mods now install to the correct location** — `.dazip` archives that use the standard `Contents/addins/` and `Contents/packages/` layout are now extracted correctly. The add-in UID is also properly read from `Manifest.xml` (capital M) and `<AddInItem>` elements.
- **Manage Games dialog no longer opens on every launch** — the dialog now only auto-opens when a genuinely new, unconfirmed game is detected. Dismissing it hides that game so it does not re-prompt next time.
- **Manage Games OK button now visible** — the OK button in the Manage Games dialog is now reliably shown when the game list page is active, including on first open.
- **DAZIP mods now install correctly** — `.dazip` archives (Dragon Age: Origins) are now installed to `AddIns/<UID>/` (matching DAUpdater behaviour) and automatically registered in `Settings/Addins.xml` so the game loads them. Loose override files continue to go to `packages/core/override/`. Previously, DAZIP contents were incorrectly dumped flat into `override/` without any registration.
- **Dragon Age: Origins mod deploy path corrected** — mods are now deployed to the Wine prefix user directory (`steamuser/Documents/BioWare/Dragon Age/packages/core/override`) instead of inside the game's installation folder.
- **No more spurious "profile could not be created" error** — switching to a newly confirmed game no longer shows a profile error because the game is now guaranteed to be saved to the database before the profile is created.
- **Newly detected games prompt before being managed** — if Deployd detects a game that has not been confirmed yet (e.g. after installing a new game), the Manage Games dialog now opens automatically so you can review and approve it before it appears in the list.
- **Plugin reordering no longer blocked by unrelated ESPs** — case-insensitive master lookups now correctly resolve to the earliest-loading copy of a plugin, preventing false "master must load first" errors when vanilla and managed plugins share the same filename (different case).
- **Update check no longer flags optional files as outdated** — optional, patch, and texture files are now compared against their own specific Nexus file version rather than the main file's version, eliminating false update notifications.

### Improved

- **FOMOD auto-selection respects your modlist** — when the FOMOD installer determines default selections, it now evaluates `<dependencyType>` conditions against your active plugin list. Options that require a DLC or another mod (e.g. a patch for Dawnguard) are automatically marked Recommended when that plugin is present, and Optional when it is not.
- **Smoother transitions during mod installation** — the progress area crossfades between idle, spinner, and progress-bar states instead of snapping instantly.
- **Loading screen on startup** — the app now shows a spinner while loading game and mod data, preventing the window from appearing frozen during startup.

---

## [0.9.5]

### Fixed

- **MCM folder no longer stripped during installation** — mods that ship with `MCM/` as their sole top-level directory (e.g. MCM Settings Manager) now install correctly to `MCM/Config/` and `MCM/Settings/` instead of bypassing the MCM folder entirely.

### Added

- **Notifications panel** — external changes and other alerts are now collected in a dedicated sidebar panel (bell icon in the headerbar) instead of a bare count button. The panel shows each notification with a description and a "Review" action, and displays an "All Caught Up" state when there is nothing pending.

### Improved

- **Headerbar overflow menu** — Purge, Create Empty Mod, Check for Updates, Manage Tools, Settings, and Reset Vanilla Baseline now live in a single "⋯" button, leaving only Deploy, Downloads, and Search visible at all times. The headerbar is much less crowded.
- **Overflow menu closes on selection** — picking any action from the overflow menu now dismisses the popover immediately.
- **Reset Vanilla Baseline always accessible** — moved from a conditional badge into the overflow menu, so it's always one click away. Still asks for confirmation before proceeding.
- **Downloads panel header** — the downloads sidebar now uses a proper header bar with title, matching the look of modern GNOME panels. The sort dropdown sits on the left and the scan/close buttons on the right.
- **Downloads button reflects panel state** — the Downloads toggle in the headerbar now lights up when the panel is open, consistent with the Search toggle.
- **All / None buttons grouped** — the All and None buttons in the Mod Order and Plugin Order panels now appear as a connected pill group, making them easier to recognise as related controls.
- **Cleaner headerbar** — game selector and a remove button sit on the left, profile management lives in a compact dropdown, and "Add Mod" has moved next to the profile for better balance. "Rescan for games" is now in Settings → Games.
- **Tools overflow menu** — when more than three external tools are configured, the extra ones are collected into a "more tools" button so the headerbar stays tidy regardless of how many tools you add.
- **Downloads panel now has its own close button** — you can dismiss the downloads panel directly from inside the panel, without going back to the toolbar.
- **Headerbar title no longer causes the window to resize** — the game title in the headerbar now ellipsizes gracefully when long, and the window stays at its current size when buttons appear or disappear.
- **Popovers now close after selecting an option** — clicking an action in the profile popover (new, clone, delete, import/export, save mode, sync) or a tool in the overflow tools menu now dismisses the popover immediately.
- **"Deployd" title stays visible** — the app title in the headerbar no longer gets squeezed away when many controls are shown on the left.

---

## [0.9.4]

### Added

- **Mod folders accessible to tools via M:\\** — all installed mod folders are now mapped as a Wine drive (`M:\`) when launching external tools. Tools like NPC Plugin Chooser 2 that need to see all versions of conflicting files (not just the deployed winner) can be configured to read from `M:\`. Folders are named by priority and mod name (e.g. `M:\00010-Bijin_NPCs\`) and are kept in sync automatically after deploy, purge, and priority changes.

### Fixed

- **Delta deployment now actually speeds up re-deploys** — the database was still being fully cleared and rebuilt on every deploy regardless of how few mods changed. Now only the affected files are removed or inserted, making small re-deploys proportionally faster.
- **Update notifications now work for all users** — the previous release check fell back to GitHub (where no releases exist), so free users running an older version never saw the update banner. The check now uses Nexus exclusively, matching where releases are actually published.

### Improved

- **Faster deploys** — deployd now computes a diff between the current and desired state and only touches files that actually changed. Mods that were already correctly deployed are left untouched, making re-deploys much faster on large modlists when only a few mods change.
- **Internal code organization** — large source files have been split into focused modules, making the codebase easier to navigate and maintain.

---

## [0.9.3]

### Added

- **Self-update** — when a new version is detected and deployd is running as an AppImage, the update banner shows a "Download Update" button that downloads and applies the update in-place. Nexus Premium is required; non-premium users are directed to the Nexus page instead.

### Fixed

- Switching the downloads sort order while a download is in progress no longer crashes the app
- Profile creation no longer fails when the auto-generated name already exists (e.g. after deleting and re-creating profiles)
- App now restores the last used profile on startup instead of always falling back to the first alphabetical one
- Deploy and Purge now show a warning when the game folder was last deployed from a different profile, helping avoid mixed mod state when switching profiles
- External tools (xEdit, BodySlide, etc.) now launch correctly for games added from custom directories; the Wine binary is now read from Heroic config even when a custom prefix is set, and Proton is auto-detected from Steam library and `compatibilitytools.d` as a fallback

---

## [0.9.2]

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

## [0.9.1]

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

## [0.9.0]

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

## [0.1.0]

- Initial internal release
