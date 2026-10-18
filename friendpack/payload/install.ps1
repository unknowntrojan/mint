# Installs mint + the bundled profiles, mod cache and config for the current Windows user.
# Existing mint config/cache/exe are renamed to *.bak-<timestamp>, never deleted.
#
# usage: INSTALL-windows.bat [-Pak "D:\...\FSD-WindowsNoEditor.pak"]
param([string]$Pak)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$ts = Get-Date -Format 'yyyyMMdd-HHmmss'

# Same known folders the `directories` crate resolves for mint.
$roaming = [Environment]::GetFolderPath('ApplicationData')
$local = [Environment]::GetFolderPath('LocalApplicationData')
$cfgDir = Join-Path $roaming 'mint\config'
$cacheDir = Join-Path $local 'mint\cache'
$exeDir = Join-Path $local 'mint'
$exe = Join-Path $exeDir 'mint.exe'

if (-not (Test-Path -LiteralPath (Join-Path $here 'mint\config\config.json')) -or
    -not (Test-Path -LiteralPath (Join-Path $here 'bin\mint.exe'))) {
    throw 'Bundle incomplete. Extract the whole zip first, then run INSTALL-windows.bat from the extracted folder.'
}
if (Get-Process -Name mint -ErrorAction SilentlyContinue) {
    throw 'mint is running. Close it first.'
}

$LaunchFlags = @('-disablemodding', '-nosplash')

function Get-SteamRoots {
    $roots = New-Object System.Collections.Generic.List[string]
    foreach ($key in 'HKCU:\Software\Valve\Steam', 'HKLM:\SOFTWARE\WOW6432Node\Valve\Steam', 'HKLM:\SOFTWARE\Valve\Steam') {
        $props = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
        if ($props.SteamPath) { $roots.Add(($props.SteamPath -replace '/', '\')) }
        if ($props.InstallPath) { $roots.Add($props.InstallPath) }
    }
    if (${env:ProgramFiles(x86)}) { $roots.Add((Join-Path ${env:ProgramFiles(x86)} 'Steam')) }
    $roots | Where-Object { Test-Path -LiteralPath $_ -PathType Container } |
        ForEach-Object { (Resolve-Path -LiteralPath $_).ProviderPath.TrimEnd('\') } |
        Sort-Object -Unique
}

function Find-DrgPak {
    foreach ($root in Get-SteamRoots) {
        $libs = @($root)
        $vdf = Join-Path $root 'steamapps\libraryfolders.vdf'
        if (Test-Path -LiteralPath $vdf) {
            foreach ($line in [IO.File]::ReadAllLines($vdf)) {
                if ($line -match '^\s*"path"\s+"(.+)"\s*$') { $libs += ($Matches[1] -replace '\\\\', '\') }
            }
        }
        foreach ($lib in $libs) {
            $p = Join-Path $lib 'steamapps\common\Deep Rock Galactic\FSD\Content\Paks\FSD-WindowsNoEditor.pak'
            if (Test-Path -LiteralPath $p -PathType Leaf) { return $p }
        }
    }

    # Xbox app / Game Pass default install location. Unverified guess; the file picker covers misses.
    foreach ($drive in [IO.DriveInfo]::GetDrives()) {
        if ($drive.DriveType -ne 'Fixed' -or -not $drive.IsReady) { continue }
        $p = Join-Path $drive.RootDirectory.FullName 'XboxGames\Deep Rock Galactic\Content\FSD\Content\Paks\FSD-WinGDK.pak'
        if (Test-Path -LiteralPath $p -PathType Leaf) { return $p }
    }
    return $null
}

function Select-DrgPak {
    try {
        Add-Type -AssemblyName System.Windows.Forms
        $dlg = New-Object System.Windows.Forms.OpenFileDialog
        $dlg.Title = 'Select FSD-WindowsNoEditor.pak (Steam) or FSD-WinGDK.pak (Xbox / Game Pass)'
        $dlg.Filter = 'DRG main pak (FSD-*.pak)|FSD-*.pak|All files (*.*)|*.*'
        if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { return $dlg.FileName }
        return $null
    } catch {
        return (Read-Host 'Path to FSD-WindowsNoEditor.pak').Trim('"')
    }
}

function Test-DrgPak([string]$p) {
    $p -and (Test-Path -LiteralPath $p -PathType Leaf) -and ((Split-Path $p -Leaf) -like 'FSD-*.pak')
}

if ($Pak) {
    if (-not (Test-DrgPak $Pak)) { throw "Not a DRG pak: $Pak" }
} else {
    $Pak = Find-DrgPak
    if ($Pak) {
        Write-Host "Found DRG: $Pak"
    } else {
        Write-Host 'Could not find Deep Rock Galactic automatically. Pick the pak file in the dialog.'
        Write-Host '  Steam: <library>\steamapps\common\Deep Rock Galactic\FSD\Content\Paks\FSD-WindowsNoEditor.pak'
        while (-not (Test-DrgPak $Pak)) {
            $Pak = Select-DrgPak
            if (-not $Pak) { throw 'Cancelled.' }
            if (-not (Test-DrgPak $Pak)) { Write-Host "  Not FSD-*.pak: $Pak" }
        }
    }
}
$Pak = (Resolve-Path -LiteralPath $Pak).ProviderPath

# Same algorithm as drg_launch_opts.py: path-aware VDF token scan, edit only
# UserLocalConfigStore/Software/Valve/Steam/apps/548430/LaunchOptions, append missing flags,
# leave every other byte alone. Returns a status line; throws on anything unexpected.
function Set-VdfLaunchOptions([string]$path, [string[]]$flags) {
    $target = 'userlocalconfigstore/software/valve/steam/apps/548430'
    $appsPath = 'userlocalconfigstore/software/valve/steam/apps'
    $utf8 = New-Object System.Text.UTF8Encoding($false, $true)  # throw on invalid UTF-8 instead of mangling
    $raw = [IO.File]::ReadAllBytes($path)
    $text = $utf8.GetString($raw)

    $locate = {
        param([string]$t)
        $stack = New-Object System.Collections.Generic.List[string]
        $key = $null
        $r = @{ AppsOpen = -1; BlockOpen = -1; ValStart = -1; ValEnd = -1; Value = $null }
        $i = 0; $n = $t.Length
        while ($i -lt $n) {
            $c = $t[$i]
            if ([char]::IsWhiteSpace($c)) { $i++; continue }
            if ($c -eq '/' -and $i + 1 -lt $n -and $t[$i + 1] -eq '/') {
                $j = $t.IndexOf("`n", $i); $i = if ($j -lt 0) { $n } else { $j }; continue
            }
            if ($c -eq '{') {
                $stack.Add($(if ($key) { $key.ToLowerInvariant() } else { '' })); $key = $null; $i++
                $p = $stack -join '/'
                if ($p -eq $appsPath -and $r.AppsOpen -lt 0) { $r.AppsOpen = $i }
                elseif ($p -eq $target -and $r.BlockOpen -lt 0) { $r.BlockOpen = $i }
                continue
            }
            if ($c -eq '}') {
                if ($stack.Count -eq 0) { throw "unbalanced '}' at offset $i" }
                $stack.RemoveAt($stack.Count - 1); $key = $null; $i++; continue
            }
            if ($c -ne '"') { throw "unexpected '$c' at offset $i" }
            $start = $i; $j = $i + 1; $sb = New-Object System.Text.StringBuilder
            while ($j -lt $n -and $t[$j] -ne '"') {
                if ($t[$j] -eq '\' -and $j + 1 -lt $n) { $j++ }
                [void]$sb.Append($t[$j]); $j++
            }
            if ($j -ge $n) { throw "unterminated string at offset $start" }
            $i = $j + 1
            if ($null -eq $key) { $key = $sb.ToString(); continue }
            if (($stack -join '/') -eq $target -and $key -eq 'LaunchOptions' -and $r.ValStart -lt 0) {
                $r.ValStart = $start; $r.ValEnd = $i; $r.Value = $sb.ToString()
            }
            $key = $null
        }
        if ($stack.Count -ne 0) { throw "unbalanced '{'" }
        $r
    }

    $loc = & $locate $text
    $current = if ($loc.Value) { $loc.Value } else { '' }
    $have = $current -split '\s+' | ForEach-Object { $_.ToLowerInvariant() }
    $missing = @($flags | Where-Object { $have -notcontains $_.ToLowerInvariant() })
    if ($missing.Count -eq 0) { return "already set: $current" }
    $new = $(if ($current.Trim()) { $current.TrimEnd() + ' ' } else { '' }) + ($missing -join ' ')
    $esc = $new.Replace('\', '\\').Replace('"', '\"')

    $t5 = "`t" * 5; $t6 = "`t" * 6
    if ($loc.ValStart -ge 0) {
        $text = $text.Substring(0, $loc.ValStart) + "`"$esc`"" + $text.Substring($loc.ValEnd)
    } elseif ($loc.BlockOpen -ge 0) {
        $text = $text.Insert($loc.BlockOpen, "`n$t6`"LaunchOptions`"`t`t`"$esc`"")
    } elseif ($loc.AppsOpen -ge 0) {
        $text = $text.Insert($loc.AppsOpen, "`n$t5`"548430`"`n$t5{`n$t6`"LaunchOptions`"`t`t`"$esc`"`n$t5}")
    } else {
        throw 'no Software/Valve/Steam/apps section found'
    }

    [void](& $locate $text)  # re-parse: never write a file we broke
    [IO.File]::WriteAllBytes("$path.bak-$ts", $raw)
    $tmp = "$path.tmp-mint"
    [IO.File]::WriteAllText($tmp, $text, $utf8)
    [IO.File]::Replace($tmp, $path, [NullString]::Value)
    "'$current' -> '$new' (backup: localconfig.vdf.bak-$ts)"
}

function Write-ManualLaunchOptions {
    Write-Host '  Set it by hand: Steam > Library > Deep Rock Galactic > Properties > General >'
    Write-Host "  Launch Options, add: $($LaunchFlags -join ' ')"
}

function Set-DrgLaunchOptions {
    Write-Host ''
    Write-Host "Setting DRG launch options ($($LaunchFlags -join ' '))..."
    while (Get-Process -Name steam -ErrorAction SilentlyContinue) {
        $ans = Read-Host '  Steam is running (it overwrites launch options on exit). Fully exit it (tray icon > Exit), then press Enter. [s = skip]'
        if ($ans -like 's*') { Write-ManualLaunchOptions; return }
    }
    $any = $false
    foreach ($root in Get-SteamRoots) {
        $userdata = Join-Path $root 'userdata'
        if (-not (Test-Path -LiteralPath $userdata)) { continue }
        foreach ($dir in Get-ChildItem -LiteralPath $userdata -Directory | Where-Object { $_.Name -match '^[1-9][0-9]*$' }) {
            $f = Join-Path $dir.FullName 'config\localconfig.vdf'
            if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { continue }
            $any = $true
            Write-Host "  $f"
            try { Write-Host "  $(Set-VdfLaunchOptions $f $LaunchFlags)" }
            catch { Write-Host "  could not edit: $($_.Exception.Message)"; Write-ManualLaunchOptions }
        }
    }
    if (-not $any) { Write-Host '  no Steam account config found.'; Write-ManualLaunchOptions }
}

function Move-Aside([string]$p) {
    if (Test-Path -LiteralPath $p) {
        $leaf = (Split-Path $p -Leaf) + ".bak-$ts"
        Rename-Item -LiteralPath $p -NewName $leaf
        Write-Host "Backed up $p -> $leaf"
    }
}

# mint prefers the legacy drg-mod-integration dirs if they exist, which would shadow this install.
Move-Aside (Join-Path $roaming 'drg-mod-integration\config')
Move-Aside (Join-Path $local 'drg-mod-integration\cache')
Move-Aside (Join-Path $roaming 'drg-mod-integration\data')
Move-Aside $cfgDir
Move-Aside $cacheDir
Move-Aside $exe

Write-Host 'Copying config and mod cache...'
New-Item -ItemType Directory -Force -Path (Split-Path $cfgDir), (Split-Path $cacheDir), $exeDir | Out-Null
Copy-Item -LiteralPath (Join-Path $here 'mint\config') -Destination $cfgDir -Recurse
Copy-Item -LiteralPath (Join-Path $here 'mint\cache') -Destination $cacheDir -Recurse

# Literal replace keeps the rest of the file byte-identical. No BOM: serde_json rejects it.
$cfgFile = Join-Path $cfgDir 'config.json'
$needle = '"drg_pak_path": null'
$json = [IO.File]::ReadAllText($cfgFile)
if (-not $json.Contains($needle)) { throw "Unexpected config.json format in $cfgFile" }
$json = $json.Replace($needle, '"drg_pak_path": ' + (ConvertTo-Json $Pak))
[IO.File]::WriteAllText($cfgFile, $json, (New-Object System.Text.UTF8Encoding $false))

Copy-Item -LiteralPath (Join-Path $here 'bin\mint.exe') -Destination $exe
Unblock-File -LiteralPath $exe  # drop the downloaded-from-internet mark so SmartScreen doesn't nag

$shell = New-Object -ComObject WScript.Shell
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) 'mint.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'mint.lnk'
foreach ($lnkPath in $startMenu, $desktop) {
    $lnk = $shell.CreateShortcut($lnkPath)
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = $exeDir
    $lnk.Description = 'Deep Rock Galactic mod integration'
    $lnk.Save()
}

if ((Split-Path $Pak -Leaf) -eq 'FSD-WinGDK.pak') {
    Write-Host ''
    Write-Host "Xbox / Game Pass version: no Steam launch options to set. Needed flags: $($LaunchFlags -join ' ')"
} else {
    Set-DrgLaunchOptions
}

Write-Host ''
Write-Host 'Done.'
Write-Host "  mint.exe: $exe  (Start Menu + Desktop shortcut)"
Write-Host "  config:   $cfgDir"
Write-Host "  cache:    $cacheDir"
Write-Host "  DRG pak:  $Pak"
Write-Host ''
Write-Host 'Start mint, pick a profile, and hit "Install mods" while DRG is closed.'
