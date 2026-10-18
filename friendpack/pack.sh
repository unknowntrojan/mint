#!/usr/bin/env bash
# Build the two friendpack downloads:
#   mint-installer-DATE.zip  mint (Linux + Windows), the Oodle libraries and the install scripts.
#                            Small; re-post it whenever mint changes.
#   mint-mods-DATE.zip       your config (incl. the mod.io OAuth token), profiles (mod_data.json)
#                            and mod cache. Big; friends only need it once. The DRG pak path is
#                            nulled out; the installer fills it in per machine. Your publish key
#                            for the preset server is left out.
set -euo pipefail

usage() {
	cat <<EOF
usage: $0 [-o DIR] [--no-installer | --no-mods] [--profile NAME]... [--oodle-dir DIR]
          [--linux-bin PATH] [--win-exe PATH]

  -o DIR            output directory (default: current directory)
  --no-installer    only build mint-mods-DATE.zip
  --no-mods         only build mint-installer-DATE.zip
  --profile NAME    only include this profile in the mods pack (repeatable; the first one
                    becomes active). Default: all profiles.
  --oodle-dir DIR   directory holding $OODLE_LINUX and $OODLE_WIN
                    (default: $repo/friendpack/oodle). Checked against the SHA-256s that
                    upstream oodle_loader pins. mint never downloads them itself.
  --linux-bin PATH  use this Linux mint binary instead of building from this repo
  --win-exe PATH    use this mint.exe instead of building from this repo

Without --linux-bin / --win-exe the binaries are built from this repo with cargo.
Building (either target) needs x86_64-w64-mingw32-gcc (Fedora: mingw64-gcc), because the
hook DLL is always cross-compiled for Windows/Proton.
EOF
}

OODLE_LINUX=liboo2corelinux64.so.9
OODLE_LINUX_SHA256=ed7e98f70be1254a80644efd3ae442ff61f854a2fe9debb0b978b95289884e9c
OODLE_WIN=oo2core_9_win64.dll
OODLE_WIN_SHA256=6f5d41a7892ea6b2db420f2458dad2f84a63901c9a93ce9497337b16c195f457

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
payload=$repo/friendpack/payload
xdg() { case ${1:-} in /*) printf '%s' "$1" ;; *) printf '%s' "$2" ;; esac; }
cfg_src=$(xdg "${XDG_CONFIG_HOME:-}" "$HOME/.config")/mint
cache_src=$(xdg "${XDG_CACHE_HOME:-}" "$HOME/.cache")/mint

outdir=$PWD
date=$(date +%Y%m%d)
oodle_dir=$repo/friendpack/oodle
linux_bin= win_exe=
do_installer=1 do_mods=1
profiles=()
while (($#)); do
	case $1 in
	-o) outdir=$(realpath -m -- "$2"); shift 2 ;;
	--no-installer) do_installer=0; shift ;;
	--no-mods) do_mods=0; shift ;;
	--profile) profiles+=("$2"); shift 2 ;;
	--oodle-dir) oodle_dir=$(realpath -- "$2"); shift 2 ;;
	--linux-bin) linux_bin=$(realpath -- "$2"); shift 2 ;;
	--win-exe) win_exe=$(realpath -- "$2"); shift 2 ;;
	-h | --help) usage; exit 0 ;;
	*) usage >&2; exit 2 ;;
	esac
done
((do_installer || do_mods)) || { echo "nothing to build" >&2; exit 2; }
mkdir -p "$outdir"

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT

check_oodle() {
	local f=$oodle_dir/$1
	if [[ ! -f $f ]]; then
		echo "missing $f" >&2
		echo "  mint no longer downloads Oodle; put $1 (sha256 $2) in $oodle_dir" >&2
		exit 1
	fi
	echo "$2  $f" | sha256sum -c --quiet - || { echo "unexpected sha256 for $f" >&2; exit 1; }
}

build_installer() {
	check_oodle "$OODLE_LINUX" "$OODLE_LINUX_SHA256"
	check_oodle "$OODLE_WIN" "$OODLE_WIN_SHA256"

	local root=$stage/mint-installer target_dir
	mkdir -p "$root/bin"
	if [[ -z $linux_bin || -z $win_exe ]]; then
		target_dir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$repo/Cargo.toml" |
			python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
	fi
	if [[ -z $linux_bin ]]; then
		cargo build --release --manifest-path "$repo/Cargo.toml"
		linux_bin=$target_dir/release/mint
	fi
	if [[ -z $win_exe ]]; then
		cargo build --release --manifest-path "$repo/Cargo.toml" --target x86_64-pc-windows-gnu
		win_exe=$target_dir/x86_64-pc-windows-gnu/release/mint.exe
	fi
	install -m755 "$linux_bin" "$root/bin/mint"
	install -m644 "$win_exe" "$root/bin/mint.exe"
	install -m644 "$oodle_dir/$OODLE_LINUX" "$oodle_dir/$OODLE_WIN" "$root/bin/"

	install -m755 "$payload/install.sh" "$root/"
	install -m644 "$payload/install.ps1" "$payload/INSTALL-windows.bat" "$payload/README.txt" \
		"$payload/drg_launch_opts.py" "$root/"

	local out=$outdir/mint-installer-$date.zip
	rm -f -- "$out"
	(cd "$stage" && zip -qr "$out" mint-installer)
	echo "wrote $out ($(du -h "$out" | cut -f1))"
	sha256sum "$root/bin/mint" "$root/bin/mint.exe" | sed "s#$root/#  #"
}

build_mods() {
	local f
	for f in "$cfg_src/config.json" "$cfg_src/mod_data.json" "$cache_src/cache.json" "$cache_src/blobs"; do
		[[ -e $f ]] || { echo "missing $f" >&2; exit 1; }
	done
	if pgrep -x mint >/dev/null; then
		echo "mint is running; close it first (it rewrites config on exit)" >&2
		exit 1
	fi

	local root=$stage/mint-mods
	mkdir -p "$root/config" "$root/cache"
	python3 -I - "$cfg_src/config.json" "$root/config/config.json" <<'EOF'
import json, sys
cfg = json.load(open(sys.argv[1], encoding="utf-8"))
if "drg_pak_path" not in cfg:
    sys.exit("config.json has no drg_pak_path key; unexpected format")
cfg["drg_pak_path"] = None  # install scripts replace the literal `"drg_pak_path": null`
cfg.pop("publish_key", None)  # lets whoever holds it publish presets; it stays with you
with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as f:
    json.dump(cfg, f, indent=2)
EOF
	python3 -I - "$cfg_src/mod_data.json" "$root/config/mod_data.json" "${profiles[@]}" <<'EOF'
import json, sys
src, dst, wanted = sys.argv[1], sys.argv[2], sys.argv[3:]
data = json.load(open(src, encoding="utf-8"))
if data.get("version") != "0.1.0":
    sys.exit("mod_data.json version %r; only 0.1.0 is supported" % data.get("version"))
if wanted:
    missing = [p for p in wanted if p not in data["profiles"]]
    if missing:
        sys.exit("unknown profile(s): %s (have: %s)" % (", ".join(missing), ", ".join(data["profiles"])))
    data["profiles"] = {p: data["profiles"][p] for p in wanted}
    used = {m["group_name"] for p in data["profiles"].values() for m in p["mods"] if "group_name" in m}
    data["groups"] = {g: m for g, m in data["groups"].items() if g in used}
    data["active_profile"] = wanted[0]
print("profiles: %s (active: %s)" % (", ".join(data["profiles"]), data["active_profile"]))
with open(dst, "w", encoding="utf-8", newline="\n") as f:
    json.dump(data, f, indent=2)
EOF
	cp --reflink=auto -- "$cache_src/cache.json" "$root/cache/"
	cp -r --reflink=auto -- "$cache_src/blobs" "$root/cache/"
	chmod 600 "$root/config/"*.json

	local out=$outdir/mint-mods-$date.zip
	rm -f -- "$out"
	(cd "$stage" && zip -qr "$out" mint-mods)
	echo "wrote $out ($(du -h "$out" | cut -f1))"
	echo "NOTE: the mods pack contains your mod.io OAuth token. Share it only with people you trust."
}

((do_installer)) && build_installer
((do_mods)) && build_mods
exit 0
