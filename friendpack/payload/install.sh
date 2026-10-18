#!/usr/bin/env bash
# Installs or updates mint for the current Linux user, and (once) imports the mods pack:
# profiles, mod cache and config from mint-mods-*.zip.
#
# - mint itself, its Oodle library, the app-menu entry and DRG's launch options are set up on
#   every run, so running a newer installer just updates mint.
# - The mods pack is imported only if it hasn't been before (see SENTINEL). It is looked for next
#   to this installer, one folder up, and in Downloads; it doesn't need to be extracted.
# - Anything replaced is moved aside to *.bak-<timestamp> (the binary to mint.old), never deleted.
#
# usage: ./install.sh [--pak /path/to/FSD-WindowsNoEditor.pak] [--mods /path/to/mint-mods-X.zip]
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ts=$(date +%Y%m%d-%H%M%S)
pak= mods=

while (($#)); do
	case $1 in
	--pak) pak=$2; shift 2 ;;
	--mods) mods=$(realpath -- "$2"); shift 2 ;;
	-h | --help) sed -n '2,12p' "$0"; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

# Same resolution as the `directories` crate mint uses: XDG vars only count if absolute.
xdg() { case ${1:-} in /*) printf '%s' "$1" ;; *) printf '%s' "$2" ;; esac; }
config_home=$(xdg "${XDG_CONFIG_HOME:-}" "$HOME/.config")
cache_home=$(xdg "${XDG_CACHE_HOME:-}" "$HOME/.cache")
data_home=$(xdg "${XDG_DATA_HOME:-}" "$HOME/.local/share")

[[ -f $here/bin/mint ]] ||
	{ echo "installer incomplete; run this from the extracted mint-installer folder" >&2; exit 1; }

if pgrep -x mint >/dev/null; then
	echo "mint is running; close it first" >&2
	exit 1
fi

# Steam rewrites its config (incl. DRG's launch options) when it exits, so it has to be closed
# before anything is changed. Waits until it is.
wait_for_steam_closed() {
	while pgrep -x steam >/dev/null; do
		if [[ ! -t 0 ]]; then
			echo "Steam is running. Close it completely (tray icon > Exit) and run this again." >&2
			exit 1
		fi
		echo "Steam is running. You must close Steam completely (tray icon > Exit)."
		read -r -s -n1 -p "Press any key to continue..."
		echo
	done
}
wait_for_steam_closed

# Written next to config.json once the mods pack has been imported successfully.
SENTINEL=$config_home/mint/friendpack-installed
LAUNCH_FLAGS=(-disablemodding -nosplash)
OODLE=liboo2corelinux64.so.9
launch_ok=1

# Existing Steam install roots, symlinks (~/.steam/steam etc.) collapsed.
steam_roots() {
	local r
	for r in \
		"$HOME/.local/share/Steam" \
		"$HOME/.steam/steam" \
		"$HOME/.steam/root" \
		"$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam" \
		"$HOME/snap/steam/common/.local/share/Steam"; do
		[[ -d $r ]] && realpath -- "$r"
	done | awk '!seen[$0]++'
}

find_pak() {
	local root vdf lib p
	while IFS= read -r root; do
		vdf=$root/steamapps/libraryfolders.vdf
		[[ -f $vdf ]] || continue
		while IFS= read -r lib; do
			lib=${lib//\\\\/\\}
			p="$lib/steamapps/common/Deep Rock Galactic/FSD/Content/Paks/FSD-WindowsNoEditor.pak"
			[[ -f $p ]] && { printf '%s' "$p"; return 0; }
		done < <(printf '%s\n' "$root"; sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$vdf")
	done < <(steam_roots)
	return 1
}

# Newest mint-mods-*.zip, or extracted mint-mods folder, near the installer or in Downloads.
find_mods() {
	local downloads d
	downloads=$(xdg-user-dir DOWNLOAD 2>/dev/null) || downloads=
	[[ -n $downloads && $downloads != "$HOME" ]] || downloads=$HOME/Downloads
	for d in "$here" "$here/.." "$downloads"; do
		[[ -d $d ]] || continue
		find "$d" -maxdepth 1 -type f -name 'mint-mods-*.zip' -printf '%T@ %p\n'
		find "$d" -maxdepth 3 -type f -path '*/mint-mods/config/config.json' -printf '%T@ %h\n' |
			sed 's#/config$##'
	done 2>/dev/null | sort -rn | head -n1 | cut -d' ' -f2-
}

manual_launch_opts() {
	launch_ok=0
	echo "  Set it by hand: Steam > Library > Deep Rock Galactic > Properties > General >"
	echo "  Launch Options, add: ${LAUNCH_FLAGS[*]}"
}

# Adds LAUNCH_FLAGS to DRG's launch options for every Steam account on this machine.
set_launch_opts() {
	echo
	echo "Setting DRG launch options (${LAUNCH_FLAGS[*]})..."
	if ! command -v python3 >/dev/null; then
		echo "  python3 not found, can't edit Steam's config."
		manual_launch_opts
		return
	fi
	wait_for_steam_closed
	local root f any=0
	while IFS= read -r root; do
		for f in "$root"/userdata/[1-9]*/config/localconfig.vdf; do
			[[ -f $f ]] || continue
			any=1
			echo "  $f"
			python3 -I "$here/drg_launch_opts.py" "$f" ".bak-$ts" "${LAUNCH_FLAGS[@]}" || manual_launch_opts
		done
	done < <(steam_roots)
	if ((!any)); then
		echo "  no Steam account config found."
		manual_launch_opts
	fi
}

# Accepts the pak itself or the DRG install folder.
normalize_pak() {
	local p=${1/#\~/$HOME}
	[[ -d $p ]] && p="$p/FSD/Content/Paks/FSD-WindowsNoEditor.pak"
	[[ -f $p && $(basename -- "$p") == FSD-*.pak ]] || return 1
	realpath -- "$p"
}

move_aside() {
	[[ -e $1 || -L $1 ]] || return 0
	mv -- "$1" "$1.bak-$ts"
	echo "backed up $1 -> $1.bak-$ts"
}

# --- 1. mods pack (once) ---
staging=
trap 'if [[ -n $staging ]]; then rm -rf -- "$staging"; fi' EXIT

import_mods() {
	local src=$1
	echo "Importing mods pack: $src"

	if [[ -n $pak ]]; then
		pak=$(normalize_pak "$pak") || { echo "not a DRG pak: $pak" >&2; exit 1; }
	elif pak=$(find_pak); then
		echo "found DRG: $pak"
	else
		[[ -t 0 ]] || { echo "DRG not found; rerun with --pak PATH" >&2; exit 1; }
		echo "Could not find Deep Rock Galactic automatically."
		while :; do
			read -r -e -p "Path to DRG install folder or FSD-WindowsNoEditor.pak: " ans
			pak=$(normalize_pak "$ans") && break
			echo "  not found / not FSD-*.pak, try again"
		done
	fi

	# Unpack next to the destination, so the big cache is moved into place instead of copied.
	mkdir -p "$cache_home"
	staging=$(mktemp -d "$cache_home/.mint-import-XXXXXX")
	if [[ -f $src ]]; then
		echo "  unpacking..."
		if command -v python3 >/dev/null; then
			python3 -I -m zipfile -e "$src" "$staging"
		elif command -v unzip >/dev/null; then
			unzip -q "$src" -d "$staging"
		else
			echo "need python3 or unzip to unpack $src" >&2
			exit 1
		fi
		src=$staging/mint-mods
	fi
	[[ -f $src/config/config.json && -f $src/config/mod_data.json && -d $src/cache ]] ||
		{ echo "not a mint mods pack: $src" >&2; exit 1; }

	# mint prefers the legacy drg-mod-integration dirs if they exist, which would shadow this.
	move_aside "$config_home/drg-mod-integration"
	move_aside "$cache_home/drg-mod-integration"
	move_aside "$data_home/drg-mod-integration"
	move_aside "$config_home/mint"
	move_aside "$cache_home/mint"
	mkdir -p "$config_home"
	if [[ $src == "$staging"/* ]]; then
		mv -- "$src/config" "$config_home/mint"
		mv -- "$src/cache" "$cache_home/mint"
	else
		cp -r --reflink=auto -- "$src/config" "$config_home/mint"
		cp -r --reflink=auto -- "$src/cache" "$cache_home/mint"
	fi

	local cfg=$config_home/mint/config.json needle='"drg_pak_path": null' esc json
	esc=${pak//\\/\\\\}
	esc=${esc//\"/\\\"}
	json=$(<"$cfg")
	[[ $json == *"$needle"* ]] || { echo "unexpected config.json format" >&2; exit 1; }
	printf '%s\n' "${json/"$needle"/"\"drg_pak_path\": \"$esc\""}" >"$cfg"
	chmod 600 "$config_home/mint/"*.json

	: >"$SENTINEL"
	echo "  mods pack imported."
}

if [[ -e $SENTINEL ]]; then
	echo "Mods pack already imported ($(date -r "$SENTINEL" '+%Y-%m-%d %H:%M')), keeping your profiles and mods."
else
	[[ -n $mods ]] || mods=$(find_mods)
	if [[ -n $mods ]]; then
		import_mods "$mods"
	elif [[ -f $config_home/mint/config.json ]]; then
		echo "No mods pack (mint-mods-*.zip) found; keeping your existing mint setup."
	else
		cat >&2 <<EOF
Couldn't find the mods pack (mint-mods-....zip).
Download it into the same folder as this installer, or into Downloads, and run this again.
EOF
		exit 1
	fi
fi

# --- 2. mint itself (every run) ---
echo
mkdir -p "$data_home/mint"
bin=$data_home/mint/mint
if [[ -e $bin ]] && ! cmp -s "$here/bin/mint" "$bin"; then
	mv -f -- "$bin" "$bin.old"
fi
install -m755 "$here/bin/mint" "$bin"
# mint loads Oodle (needed for some mods) from next to its binary and never downloads it.
install -m644 "$here/bin/$OODLE" "$data_home/mint/$OODLE"
echo "installed $bin"

desktop=$data_home/applications/mint.desktop
if [[ -e $desktop ]] && ! grep -qx 'Comment=Deep Rock Galactic mod integration' "$desktop"; then
	move_aside "$desktop"
fi
mkdir -p "$data_home/applications"
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=mint
Comment=Deep Rock Galactic mod integration
Exec="$bin"
Terminal=false
Categories=Game;
EOF

# --- 3. DRG launch options (every run, no-op once set) ---
set_launch_opts

if ((!launch_ok)); then
	cat <<EOF

mint is installed, but DRG's launch options could not be set automatically (see above).
Set them by hand, or close Steam and run this installer again.
EOF
	exit 1
fi

cat <<EOF

done.
  mint:    $bin  (also in your app menu as "mint")
  config:  $config_home/mint
  cache:   $cache_home/mint

Start mint, pick a profile, and hit "Install mods" while DRG is closed.
EOF
