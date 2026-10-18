#!/usr/bin/env bash
# Installs mint + the bundled profiles, mod cache and config for the current Linux user.
# Existing mint config/cache/binary are moved aside to *.bak-<timestamp>, never deleted.
#
# usage: ./install.sh [--pak /path/to/FSD-WindowsNoEditor.pak]
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ts=$(date +%Y%m%d-%H%M%S)
pak=

while (($#)); do
	case $1 in
	--pak) pak=$2; shift 2 ;;
	-h | --help) sed -n '2,6p' "$0"; exit 0 ;;
	*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

# Same resolution as the `directories` crate mint uses: XDG vars only count if absolute.
xdg() { case ${1:-} in /*) printf '%s' "$1" ;; *) printf '%s' "$2" ;; esac; }
config_home=$(xdg "${XDG_CONFIG_HOME:-}" "$HOME/.config")
cache_home=$(xdg "${XDG_CACHE_HOME:-}" "$HOME/.cache")
data_home=$(xdg "${XDG_DATA_HOME:-}" "$HOME/.local/share")

[[ -f $here/mint/config/config.json && -f $here/bin/mint ]] ||
	{ echo "bundle incomplete; run this from the extracted mint-bundle folder" >&2; exit 1; }

if pgrep -x mint >/dev/null; then
	echo "mint is running; close it first" >&2
	exit 1
fi

LAUNCH_FLAGS=(-disablemodding -nosplash)

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

manual_launch_opts() {
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
	while pgrep -x steam >/dev/null; do
		if [[ ! -t 0 ]]; then
			echo "  Steam is running (it overwrites launch options on exit), skipping."
			manual_launch_opts
			return
		fi
		read -r -p "  Steam is running. Fully exit it (tray icon > Exit), then press Enter. [s = skip] " ans
		if [[ $ans == [sS]* ]]; then
			manual_launch_opts
			return
		fi
	done
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

move_aside() {
	[[ -e $1 || -L $1 ]] || return 0
	mv -- "$1" "$1.bak-$ts"
	echo "backed up $1 -> $1.bak-$ts"
}

# mint prefers the legacy drg-mod-integration dirs if they exist, which would shadow this install.
move_aside "$config_home/drg-mod-integration"
move_aside "$cache_home/drg-mod-integration"
move_aside "$data_home/drg-mod-integration"
move_aside "$config_home/mint"
move_aside "$cache_home/mint"
move_aside "$data_home/mint/mint"

mkdir -p "$config_home" "$cache_home" "$data_home/mint"
cp -r --reflink=auto -- "$here/mint/config" "$config_home/mint"
cp -r --reflink=auto -- "$here/mint/cache" "$cache_home/mint"

cfg=$config_home/mint/config.json
needle='"drg_pak_path": null'
esc=${pak//\\/\\\\}
esc=${esc//\"/\\\"}
json=$(<"$cfg")
[[ $json == *"$needle"* ]] || { echo "unexpected config.json format" >&2; exit 1; }
printf '%s\n' "${json/"$needle"/"\"drg_pak_path\": \"$esc\""}" >"$cfg"
chmod 600 "$config_home/mint/"*.json

bin=$data_home/mint/mint
install -m755 "$here/bin/mint" "$bin"

desktop=$data_home/applications/mint.desktop
move_aside "$desktop"
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

set_launch_opts

cat <<EOF

done.
  binary:  $bin  (also in your app menu as "mint")
  config:  $config_home/mint
  cache:   $cache_home/mint
  DRG pak: $pak

Start mint, pick a profile, and hit "Install mods" while DRG is closed.
EOF
