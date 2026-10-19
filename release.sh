#!/usr/bin/env bash
# Build and publish a mint release; installed release builds update to it on their next start.
#   ./release.sh ["what changed"]
#
# Builds Linux + Windows from the committed tree with MINT_RELEASE=<next number>, uploads both
# to the preset server and signs the manifest. Needs the signing key (`mint_release keygen`,
# ~/.local/share/mint-release/signing.key), mint's publish key setting, and
# x86_64-w64-mingw32-gcc (Fedora: mingw64-gcc). Tags the commit release-<n>.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
notes=${1:-}
manifest=$repo/Cargo.toml

if [[ -n $(git -C "$repo" status --porcelain --untracked-files=no) ]]; then
	echo "commit your changes first: a release is built from the committed tree" >&2
	exit 1
fi

target_dir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" |
	python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
cargo build -q -p mint_release --manifest-path "$manifest"
tool=$target_dir/debug/mint_release

n=$("$tool" next)
echo "building release $n from $(git -C "$repo" rev-parse --short HEAD)"
export MINT_RELEASE=$n
cargo build --profile dist --manifest-path "$manifest"
cargo build --profile dist --manifest-path "$manifest" --target x86_64-pc-windows-gnu

"$tool" publish "$n" "$target_dir/dist/mint" "$target_dir/x86_64-pc-windows-gnu/dist/mint.exe" "$notes"
git -C "$repo" tag "release-$n"
echo "tagged release-$n"
