#!/usr/bin/env bash
# Build the two friendpack downloads:
#   mint-installer-DATE.zip  mint (Linux + Windows), the Oodle libraries and the install scripts.
#                            mint is the latest published release (../release.sh), so it updates
#                            itself from then on; the installer only matters for new friends.
#   mint-mods-DATE.zip       your config (incl. the mod.io OAuth token), profiles (mod_data.json)
#                            and the cached mods that a packed profile enables, with their
#                            dependencies (--all-mods: the whole mod cache). Friends only need it
#                            once; mods they turn on later are downloaded from mod.io. The DRG pak
#                            path is nulled out; the installer fills it in per machine. Your
#                            publish key for the preset server is left out.
set -euo pipefail

usage() {
	cat <<EOF
usage: $0 [-o DIR] [--no-installer | --no-mods] [--profile NAME]... [--all-mods]
          [--oodle-dir DIR] [--linux-bin PATH] [--win-exe PATH]

  -o DIR            output directory (default: current directory)
  --no-installer    only build mint-mods-DATE.zip
  --no-mods         only build mint-installer-DATE.zip
  --profile NAME    only include this profile in the mods pack (repeatable; the first one
                    becomes active). Default: all profiles.
  --all-mods        put the whole mod cache into the mods pack. Default: only the mods that
                    a packed profile enables, and their dependencies.
  --oodle-dir DIR   directory holding $OODLE_LINUX and $OODLE_WIN
                    (default: $repo/friendpack/oodle). Checked against the SHA-256s that
                    upstream oodle_loader pins. mint never downloads them itself.
  --linux-bin PATH  use this Linux mint binary instead of the latest release
  --win-exe PATH    use this mint.exe instead of the latest release

Without --linux-bin / --win-exe the binaries are the latest release on the preset server,
downloaded and checked against its signature. Only release builds update themselves; publish
one with ../release.sh first.
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
do_installer=1 do_mods=1 all_mods=0
profiles=()
while (($#)); do
	case $1 in
	-o) outdir=$(realpath -m -- "$2"); shift 2 ;;
	--no-installer) do_installer=0; shift ;;
	--no-mods) do_mods=0; shift ;;
	--profile) profiles+=("$2"); shift 2 ;;
	--all-mods) all_mods=1; shift ;;
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

	local root=$stage/mint-installer target_dir release
	mkdir -p "$root/bin"
	if [[ -z $linux_bin || -z $win_exe ]]; then
		target_dir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$repo/Cargo.toml" |
			python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
		cargo build -q -p mint_release --manifest-path "$repo/Cargo.toml"
		release=$("$target_dir/debug/mint_release" download "$stage/release")
		echo "mint: release $release"
		linux_bin=${linux_bin:-$stage/release/mint}
		win_exe=${win_exe:-$stage/release/mint.exe}
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
    data["active_profile"] = wanted[0]
print("profiles: %s (active: %s)" % (", ".join(data["profiles"]), data["active_profile"]))
with open(dst, "w", encoding="utf-8", newline="\n") as f:
    json.dump(data, f, indent=2)
EOF
	cp --reflink=auto -- "$cache_src/cache.json" "$root/cache/"
	if ((all_mods)); then
		cp -r --reflink=auto -- "$cache_src/blobs" "$root/cache/"
	else
		# cache.json stays complete: mint downloads any mod whose blob is missing.
		python3 -I - "$cache_src/cache.json" "$root/config/mod_data.json" "$cache_src/blobs" \
			>"$stage/blobs.txt" <<'EOF'
import json, os, re, sys
cache_path, data_path, blob_dir = sys.argv[1:]
modio = json.load(open(cache_path, encoding="utf-8"))["cache"].get("modio", {})
mods, names, deps, blob_of = (modio.get(k, {}) for k in ("mods", "mod_id_map", "dependencies", "modfile_blobs"))
data = json.load(open(data_path, encoding="utf-8"))

# modfiles mint would integrate for a mod (unpinned: its latest) and for its dependencies
modfiles, seen = set(), set()
def add(mod_id):
    if mod_id in seen:
        return
    seen.add(mod_id)
    m = mods.get(mod_id, {})
    if m.get("latest_modfile"):
        modfiles.add(str(m["latest_modfile"]))
    if m.get("modfiles"):
        modfiles.add(str(m["modfiles"][-1]["id"]))
    for d in deps.get(mod_id, []):
        add(str(d))

for profile in data["profiles"].values():
    for entry in profile["mods"]:
        hit = re.search(r"mod\.io/g/drg/m/([^/#?]+)(?:#(\d+)(?:/(\d+))?)?", entry["spec"]["url"])
        if not entry["enabled"] or not hit:
            continue
        if hit[3]:
            modfiles.add(hit[3])
        add(hit[2] or str(names.get(hit[1], "")))

wanted = {blob_of[f] for f in modfiles if f in blob_of}
modio_blobs = set(blob_of.values())
blobs = [b for b in sorted(os.listdir(blob_dir)) if not b.startswith(".")]  # .x: unfinished
total = kept = n = 0
for name in blobs:
    size = os.path.getsize(os.path.join(blob_dir, name))
    total += size
    # blobs of other providers stay
    if name in wanted or name not in modio_blobs:
        print(name)
        kept += size
        n += 1
print("mod cache: %d of %d files, %.0f of %.0f MB (mods enabled in a packed profile; --all-mods for all)"
      % (n, len(blobs), kept / 1e6, total / 1e6), file=sys.stderr)
EOF
		mkdir -p "$root/cache/blobs"
		local blobs
		mapfile -t blobs <"$stage/blobs.txt"
		if ((${#blobs[@]})); then
			(cd "$cache_src/blobs" && cp --reflink=auto -t "$root/cache/blobs" -- "${blobs[@]}")
		fi
	fi
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
