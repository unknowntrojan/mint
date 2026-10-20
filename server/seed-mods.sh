#!/usr/bin/env bash
# Copy the mod files in your mint cache straight into the server's mod mirror over ssh:
#   server/seed-mods.sh [SSH_HOST [DATA_DIR]]   (default root@lilnasx /data/docker/mint/data)
#
# LAN speed and no Cloudflare size limit, unlike `mint_release seed-mods`, which uploads through
# the public API. Files the mirror has already are skipped, every file is checked against the
# SHA-256 mint stores it under, and everything under mods/ ends up owned like mods/ itself (the
# container's user). Deploy the server first; it creates mods/.
set -euo pipefail

host=${1:-root@lilnasx}
data=${2:-/data/docker/mint/data}
cache=$(case ${XDG_CACHE_HOME:-} in /*) printf '%s' "$XDG_CACHE_HOME" ;; *) printf '%s' "$HOME/.cache" ;; esac)/mint
[[ -f $cache/cache.json ]] || { echo "no mint cache at $cache" >&2; exit 1; }

existing=$(mktemp)
trap 'rm -f -- "$existing"' EXIT
# the login shell on the NAS isn't bash; feed every remote script to bash explicitly
ssh "$host" bash -s >"$existing" <<REMOTE
set -euo pipefail
cd '$data/mods' 2>/dev/null || { echo "no $data/mods; deploy the server first" >&2; exit 1; }
find . -mindepth 2 -maxdepth 2 -name '*.json' | sed 's#^\./##; s#\.json\$##'
REMOTE

python3 -I - "$cache" "$existing" <<'EOF' |
import hashlib, io, json, os, sys, tarfile, time

cache_dir, existing_path = sys.argv[1:]
existing = set(open(existing_path).read().split())  # "<mod>/<modfile>"
modio = json.load(open(os.path.join(cache_dir, "cache.json"), encoding="utf-8"))["cache"].get("modio", {})
mod_of = {str(f["id"]): mod for mod, m in modio.get("mods", {}).items() for f in m.get("modfiles", [])}

def readable(t):
    t.mode = 0o644
    return t

out = tarfile.open(fileobj=sys.stdout.buffer, mode="w|")
now = time.time()
dirs, sent, size, present, bad = set(), 0, 0, 0, []
for modfile, blob in sorted(modio.get("modfile_blobs", {}).items(), key=lambda kv: int(kv[0])):
    mod = mod_of.get(modfile)
    path = os.path.join(cache_dir, "blobs", blob)
    if mod is None or not os.path.isfile(path):
        continue
    if f"{mod}/{modfile}" in existing:
        present += 1
        continue
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    if h.hexdigest() != blob:
        bad.append(f"mod {mod} file {modfile}")
        continue
    if mod not in dirs:
        d = tarfile.TarInfo(f"mods/{mod}")
        d.type, d.mode, d.mtime = tarfile.DIRTYPE, 0o755, now
        out.addfile(d)
        dirs.add(mod)
    # the file first: the server ignores files without their .json
    out.add(path, arcname=f"mods/{mod}/{modfile}", filter=readable)
    meta = json.dumps(
        {"mod_id": int(mod), "modfile_id": int(modfile), "size": os.path.getsize(path), "sha256": blob},
        indent=2,
    ).encode()
    t = tarfile.TarInfo(f"mods/{mod}/{modfile}.json")
    t.size, t.mode, t.mtime = len(meta), 0o644, now
    out.addfile(t, io.BytesIO(meta))
    sent += 1
    size += os.path.getsize(path)
out.close()
for b in bad:
    print(f"skipped {b}: doesn't match its SHA-256, delete it from the cache", file=sys.stderr)
print(f"sending {sent} files ({size / 1e6:.0f} MB); {present} were there already", file=sys.stderr)
EOF
	ssh "$host" "bash -c 'cd $data && tar --no-same-owner -xf - && chown -R \"\$(stat -c %u:%g mods)\" mods'"
echo "done"
