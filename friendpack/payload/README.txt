mint: Deep Rock Galactic mod loader, preconfigured with modlists.

There are two downloads:
  mint-installer-DATE.zip   mint itself + the installer. Small, gets updated now and then.
  mint-mods-DATE.zip        the mods, modlists and settings. Big, you only need it once.
                            Don't extract it; the installer finds it.

First time:
1. Download both into the same folder (e.g. Downloads).
2. Extract mint-installer-DATE.zip (right click > Extract All), not just double-click into it.
3. Run the installer from the extracted folder:
     Windows: double-click INSTALL-windows.bat
              If Windows says "Windows protected your PC": More info > Run anyway.
     Linux:   ./install.sh        (or: bash install.sh)
   It finds Deep Rock Galactic via Steam. If it can't, it asks you for the
   FSD-WindowsNoEditor.pak (Steam) or FSD-WinGDK.pak (Xbox/Game Pass) file.
   It also adds "-disablemodding -nosplash" to DRG's Steam launch options (keeping any
   you already have). Steam has to be fully closed for that; it will ask. If it can't,
   set them by hand: Steam > Library > Deep Rock Galactic > Properties > General >
   Launch Options.
4. Start mint (Start Menu / Desktop / app menu), pick a profile, click "Install mods".
   DRG must be closed while installing/uninstalling mods.

Updating mint later: download only the new mint-installer zip, extract it, run the installer
again. Your mods and modlists stay as they are; the mods zip is only imported the first time.

Any existing mint setup is renamed to *.bak-<timestamp>, not deleted.
To go back to the official mod integration: "Uninstall mods" in mint, then start DRG normally.

Don't share the mods zip further: it contains a personal mod.io access token.
