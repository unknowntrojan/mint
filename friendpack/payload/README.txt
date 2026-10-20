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
   Steam has to be fully closed first (tray icon > Exit); the installer waits until it is.
   It adds "-disablemodding -nosplash" to DRG's Steam launch options (keeping any you
   already have). If that fails, set them by hand: Steam > Library > Deep Rock Galactic >
   Properties > General > Launch Options.
4. Start mint (Start Menu / Desktop / app menu), pick a profile, click "Install mods".
   DRG must be closed while installing/uninstalling mods.

Updating: mint updates itself. When there's a new version it downloads it on start and
restarts by itself. Running a newer installer again also works: your mods and modlists stay as
they are; the mods zip is only imported the first time.

Any existing mint setup is renamed to *.bak-<timestamp>, not deleted.
To go back to the official mod integration: "Uninstall mods" in mint, then start DRG normally.

Don't share the mods zip further: it contains a personal mod.io access token.
