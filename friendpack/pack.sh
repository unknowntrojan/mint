#!/usr/bin/env bash
# Build a shareable mint bundle: Linux + Windows binaries, your profiles (mod_data.json),
# your mod cache (cache.json + blobs) and your config (incl. the mod.io OAuth token).
# The DRG pak path is nulled out; the bundled install scripts fill it in per machine.
set -euo pipefail

usage() {
	cat <<EOF
usage: $0 [-o OUT.zip] [--linux-bin PATH] [--win-exe PATH | --fetch-win]

  -o PATH           output zip (default: ./mint-bundle-YYYYMMDD.zip)
  --linux-bin PATH  use this Linux mint binary instead of building from this repo
  --win-exe PATH    use this mint.exe instead of building from this repo
  --fetch-win       download mint.exe from the Wasserkleber/mintfixed $WIN_RELEASE release

Without --linux-bin / --win-exe the binaries are built from this repo with cargo.
Building (either target) needs x86_64-w64-mingw32-gcc (Fedora: mingw64-gcc), because the
hook DLL is always cross-compiled for Windows/Proton.
EOF
}

WIN_RELEASE=mintfixed_1.3
WIN_URL=https://github.com/Wasserkleber/mintfixed/releases/download/$WIN_RELEASE/$WIN_RELEASE.exe

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
payload=$repo/friendpack/payload
xdg() { case ${1:-} in /*) printf '%s' "$1" ;; *) printf '%s' "$2" ;; esac; }
cfg_src=$(xdg "${XDG_CONFIG_HOME:-}" "$HOME/.config")/mint
cache_src=$(xdg "${XDG_CACHE_HOME:-}" "$HOME/.cache")/mint

out=$PWD/mint-bundle-$(date +%Y%m%d).zip
linux_bin= win_exe= fetch_win=0
while (($#)); do
	case $1 in
	-o) out=$(realpath -m -- "$2"); shift 2 ;;
	--linux-bin) linux_bin=$(realpath -- "$2"); shift 2 ;;
	--win-exe) win_exe=$(realpath -- "$2"); shift 2 ;;
	--fetch-win) fetch_win=1; shift ;;
	-h | --help) usage; exit 0 ;;
	*) usage >&2; exit 2 ;;
	esac
done

for f in "$cfg_src/config.json" "$cfg_src/mod_data.json" "$cache_src/cache.json" "$cache_src/blobs"; do
	[[ -e $f ]] || { echo "missing $f" >&2; exit 1; }
done
if pgrep -x mint >/dev/null; then
	echo "mint is running; close it first (it rewrites config on exit)" >&2
	exit 1
fi

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
root=$stage/mint-bundle
mkdir -p "$root/bin" "$root/mint/config" "$root/mint/cache"

# --- binaries ---
if [[ -z $linux_bin ]]; then
	cargo build --release --manifest-path "$repo/Cargo.toml"
	linux_bin=$repo/target/release/mint
fi
if [[ -z $win_exe && $fetch_win == 1 ]]; then
	curl -fL --proto '=https' --progress-bar -o "$stage/mint.exe" "$WIN_URL"
	win_exe=$stage/mint.exe
elif [[ -z $win_exe ]]; then
	cargo build --release --manifest-path "$repo/Cargo.toml" --target x86_64-pc-windows-gnu
	win_exe=$repo/target/x86_64-pc-windows-gnu/release/mint.exe
fi
install -m755 "$linux_bin" "$root/bin/mint"
install -m644 "$win_exe" "$root/bin/mint.exe"

# --- config / profiles / cache ---
python3 -I - "$cfg_src/config.json" "$root/mint/config/config.json" <<'EOF'
import json, sys
cfg = json.load(open(sys.argv[1], encoding="utf-8"))
if "drg_pak_path" not in cfg:
    sys.exit("config.json has no drg_pak_path key; unexpected format")
cfg["drg_pak_path"] = None  # install scripts replace the literal `"drg_pak_path": null`
with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as f:
    json.dump(cfg, f, indent=2)
EOF
cp --reflink=auto -- "$cfg_src/mod_data.json" "$root/mint/config/"
cp --reflink=auto -- "$cache_src/cache.json" "$root/mint/cache/"
cp -r --reflink=auto -- "$cache_src/blobs" "$root/mint/cache/"
chmod 600 "$root/mint/config/"*.json

# --- install scripts ---
install -m755 "$payload/install.sh" "$root/"
install -m644 "$payload/install.ps1" "$payload/INSTALL-windows.bat" "$payload/README.txt" \
	"$payload/drg_launch_opts.py" "$root/"

rm -f -- "$out"
(cd "$stage" && zip -qr "$out" mint-bundle)

echo
echo "wrote $out ($(du -h "$out" | cut -f1))"
sha256sum "$root/bin/mint" "$root/bin/mint.exe" | sed "s#$root/##"
echo "NOTE: the bundle contains your mod.io OAuth token. Share it only with people you trust."
