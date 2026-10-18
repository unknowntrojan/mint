@echo off
if not exist "%~dp0install.ps1" (
    echo Extract the whole zip first, then run INSTALL-windows.bat from the extracted folder.
    pause
    exit /b 1
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -File "%~dp0install.ps1" %*
pause
