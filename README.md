# Deployd

![Version](https://img.shields.io/badge/version-3.3.2-blue)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux)
![License](https://img.shields.io/badge/license-GPL--3.0--only-green)
[![deployd](https://snapcraft.io/deployd/badge.svg)](https://snapcraft.io/deployd)

Deployd is a Linux mod manager. Install mod archives, download from Nexus Mods,
organize load orders, and switch between mod profiles.

> **v3.3.2** · [Release notes](CHANGELOG.md) · [Report a problem](https://gitlab.com/mattianelo/deployd/-/issues)

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
- Mass Effect Legendary Edition — **experimental**; read the [MELE guide](https://gitlab.com/mattianelo/deployd/-/wikis/mass-effect-legendary-edition) before setup

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

## User guide

The [wiki](https://gitlab.com/mattianelo/deployd/-/wikis/home) covers:

- [Profiles, history, and saves](https://gitlab.com/mattianelo/deployd/-/wikis/profiles)
- [Deployment and restoring game files](https://gitlab.com/mattianelo/deployd/-/wikis/mod-cache-and-deployment)
- [Mass Effect Legendary Edition](https://gitlab.com/mattianelo/deployd/-/wikis/mass-effect-legendary-edition)
- [External tools](https://gitlab.com/mattianelo/deployd/-/wikis/external-tools)
- [Folder access and troubleshooting](https://gitlab.com/mattianelo/deployd/-/wikis/troubleshooting)

## Help and support

[Report bugs or request features](https://gitlab.com/mattianelo/deployd/-/issues).
Include your Deployd version, Snap or AppImage, the game, what you were doing,
and any error message. Check the [release notes](CHANGELOG.md) for recent changes.

If Deployd helps you, you can [support development on Ko-fi](https://ko-fi.com/mattianelo).

Licensed under GPL-3.0-only.
