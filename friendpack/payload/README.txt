mint bundle: mod loader + modlists + mod cache, preconfigured.

1. Extract the whole zip somewhere (not just double-click into it).
2. Run the installer:
     Windows: double-click INSTALL-windows.bat
     Linux:   ./install.sh        (or: bash install.sh)
   It finds Deep Rock Galactic via Steam. If it can't, it asks you for the
   FSD-WindowsNoEditor.pak (Steam) or FSD-WinGDK.pak (Xbox/Game Pass) file.
   You can also pass it directly:
     Windows: INSTALL-windows.bat -Pak "D:\SteamLibrary\steamapps\common\Deep Rock Galactic\FSD\Content\Paks\FSD-WindowsNoEditor.pak"
     Linux:   ./install.sh --pak ~/.local/share/Steam/steamapps/common/"Deep Rock Galactic"
   It also adds "-disablemodding -nosplash" to DRG's Steam launch options (keeping any
   you already have). Steam has to be fully closed for that; it will ask. If it can't,
   set them by hand: Steam > Library > Deep Rock Galactic > Properties > General >
   Launch Options.
3. Start mint (Start Menu / Desktop / app menu), pick a profile, click "Install mods".
   DRG must be closed while installing/uninstalling mods.

Any existing mint setup is renamed to *.bak-<timestamp>, not deleted.
To go back to the official mod integration: "Uninstall mods" in mint, then start DRG normally.

Don't share this bundle further: it contains a personal mod.io access token.
