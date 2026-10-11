# Deployd

![Version](https://img.shields.io/badge/version-3.3.1-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0--only-green)
[![deployd](https://snapcraft.io/deployd/badge.svg)](https://snapcraft.io/deployd)

Deployd is a Linux mod manager. Install mod archives, download from Nexus Mods,
organize load orders, and switch between mod profiles.

> **v3.3.1** · [Release notes](CHANGELOG.md) · [Report a problem](https://gitlab.com/mattianelo/deployd/-/issues)

## Install

- **Snap:** install from the [Snap Store](https://snapcraft.io/deployd).
  Updates are managed by Snap.
- **AppImage:** download from [Nexus Mods](https://www.nexusmods.com/skyrimspecialedition/mods/174218),
  mark the file as executable in your file manager, and open it.
  Deployd notifies you when an update is available.

Deployd supports 64-bit x86 Linux systems. Games run through Wine or Proton.

## Supported games

- Skyrim Special Edition, Fallout 4, Fallout: New Vegas, and Starfield
- The Witcher, The Witcher 2, The Witcher 3, and Cyberpunk 2077
- Dragon Age: Origins
- Mass Effect Legendary Edition — **experimental**; read the [MELE notes](#mass-effect-legendary-edition) before setup

## Get started

1. **Add your game.** The welcome wizard asks for its installation folder and
   Wine or Proton prefix. Add or change games later in **Settings → Manage Games**.
   For a Steam prefix, select the numbered `compatdata` folder containing `pfx`.
2. **Install a mod.** Select the game, then drop a `.zip`, `.7z`, `.rar`, or `.dazip`
   archive onto the window, or use **+**. Choose any installer options when prompted.
3. **Enable and order mods.** Enter **Select Mode**, select mods to enable or
   disable, and drag rows to change their order. Mods lower in the list win file
   conflicts. Choose **Done** when finished.
4. **Check plugins.** Bethesda games have a separate **Plugin Order** list. Use
   its Select Mode to enable plugins and arrange their load order.
5. **Deploy.** This applies your selected profile to the game. Deploy again after
   changing enabled mods, removing mods, reordering, or switching profiles.

Installing or removing a mod from the library does not immediately change the
files in your game. **Deploy** applies those changes; **Purge** removes managed
mod files and restores protected originals while keeping your mod library.

### Downloads and installation options

Log in under **Settings → Nexus Mods**, then use **Mod Manager Download** on
Nexus Mods to send files to Deployd. The Downloads panel lets you track, pause,
resume, and install downloads.

Reinstall a mod to change its installer options. When installing a replacement,
**Replace** keeps its position in the mod order and its plugin states. Open a
mod's **Properties** to inspect files, conflicts, notes, or Nexus metadata.

Dragon Age: Origins separates **DAZIPs & Tools** from **Overrides**. Mixed
archives create separate entries, so you can enable, remove, or reinstall each
component independently. Apply those changes with **Deploy**.

## Profiles, history, and saves

Use the profile selector to create, clone, or switch mod configurations.
The header distinguishes the profile you are editing from the one currently
deployed. Selecting a profile alone does not change game files or live saves.

**Deploy options → Deployment history** keeps previous deployments, including
installed content from disabled mods. Restore an entry as a new profile, then
choose **Deploy** to apply it. History survives ordinary mod and profile removal;
it does not include saves.

Profiles can share **Global** saves or use **Profile-specific** saves. The first
profile-specific save bank starts with the current live saves. Save banks switch
when you deploy, so close the game before switching profiles or restoring saves.
Use **Profile options → Manage save backups** to create or restore backups.
**Sync** backs up outgoing live saves to their own banks. For trilogy profiles, it also
loads the selected profile’s saves across all three games without redeploying mods.
For other games, deploy the selected profile first if Sync is unavailable.

## Storage and restoring game files

Keep **Protect vanilla game files** enabled to back up originals that mods would
replace. **Purge** restores those originals and leaves unrelated files alone.
Files replaced without a backup must be recovered through your game platform's
file-verification feature.

Change a game's cache folder under **Settings → Manage Games**. Hardlink deployment
requires the cache and game to share a compatible filesystem: the same partition,
Btrfs subvolume, or ZFS dataset. MELE needs additional space for separate copies,
restoration data, and cached texture processing. Updates to texture processing
may require cached results to be rebuilt on the next deployment. Deployment history also uses disk
space; review and delete unneeded, unprotected entries from its dialog.

To start over, choose **Purge & delete all Deployd data** when removing a game.
This restores managed game files, then clears its mod cache, profiles, history,
and settings. Original game files, saves, and downloaded archives are preserved.
The other reset choices leave deployed files in place; read the confirmation
before choosing one. If cleanup is interrupted, restore any missing folder access
and retry.

Opening a game performs recovery and access checks. Full content checks run before
deployment and restoration. To check retained and deployed files yourself, use
**Deployment history → Full integrity check**. It reports problems without replacing
files; external edits may not appear in status until a content check runs.

## Mass Effect Legendary Edition

MELE support is experimental. Begin with a **clean, unmodded installation of all
three games**. Initial setup and large texture installations can take several
minutes and need substantial free disk space.

- LE1, LE2, and LE3 share trilogy profiles, with separate mod lists for each game.
  Deploy applies only to the selected game, including its portion of the save bank.
  Before changing trilogy profiles, Deployd syncs all three games to the banks
  that currently own their live saves. Manual Sync backs up outgoing saves and
  loads the selected trilogy profile’s banks for all three games, independently
  of deployed mods. A failed switch restores the outgoing saves. Deploying then
  loads the selected game’s target character bank, with rollback if deployment fails.
- Supported mods include Community Patch, content and cosmetic mods, M3TO textures
  (including ALOT and ISL), shaders, and squadmate outfits. Unsupported packages
  are reported before installation. Install dependencies before the mods needing them.
- Installer options and M3TO texture manifests accept the formatting used by
  ME3Tweaks mods, including single-option lists and commented texture manifests.
  Configuration patches can add missing LE1 config assets and LE2 DLC INI files.
- LE1 and LE2 configuration deltas support typed additions, including ESA’s
  optional New Armors for Femshep LE2 patch. Select that patch only when using
  its required armor mod, and follow the author’s installation order.
- MELE respects mod authors' DLC priorities as well as mod order. Reordering mods
  does not change their DLC priorities.
- Regenerated `PCConsoleTOC.bin` indexes in managed game content are handled
  automatically during deployment and profile switching. Failed deployments
  restore their pre-deployment contents; edits to actual mod assets still block
  replacement.
- ASI/DLL plugins require approval for each package version and game. Follow the
  author's instructions for any Proton DLL overrides. Deployd does not run
  installers or scripts supplied in archives.
- **MEM textures, portable MELE profile transfers, launcher executable replacements,
  and launcher installer choices are unsupported.** Restore existing MEM textures
  to clean game content before adding the game.
- Removing MELE from management applies to the **whole trilogy**. Full reset waits
  for all three games to purge successfully before clearing their data. If one
  fails, all three remain managed so you can retry.

Use **Group trilogy profiles…** to combine older, separate game profiles.
Unmatched profiles remain available as ungrouped profiles.

**Edit appearance…** lets you edit hair and cosmetic references in a live save,
import TSE RON or Gibbed headmorph presets, and export RON presets. Close the game
first; Deployd makes a backup before saving. There is no visual preview or asset
browser: use the mod author's asset names and install the required mods. Default
Shepard saves need an imported custom preset before individual fields can be edited.
Installing or deploying a mod never changes appearance in your saves automatically.

## External tools

Launch tools such as xEdit, LOOT, and BodySlide from the **Tools** panel. Add tools,
arguments, or a working directory through the tool manager. The selected profile
must match the deployed configuration before launching a tool.

After a tool closes, Deployd scans for external changes. Review those changes
before deploying; it does not automatically sort or deploy them. The Snap uses a
separate Wine environment for tools, which may need a download on first use.

## Folder access in Snap

Snap can only use folders it has permission to access. If a saved location becomes
unavailable, open **Settings → Manage Games → Restore access** and select the
original folder again. For Wine prefixes, select the containing folder so access
survives Proton replacing the prefix.

Access to a downloads folder does not grant access to every folder on that drive.
If a local archive cannot be opened, move it into your configured downloads folder,
scan that folder, and install it from Downloads. Follow any connection instructions
Deployd displays for external drives or tool runtimes.

## Help and support

[Report bugs or request features](https://gitlab.com/mattianelo/deployd/-/issues).
Include your Deployd version, Snap or AppImage, the game, what you were doing,
and any error message. Check the [release notes](CHANGELOG.md) for recent changes.

If Deployd helps you, you can [support development on Ko-fi](https://ko-fi.com/mattianelo).

Licensed under GPL-3.0-only.
