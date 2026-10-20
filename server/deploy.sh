#!/usr/bin/env bash
# Build mint_server's image on the NAS and (re)start it:
#   server/deploy.sh [SSH_HOST]      (default root@lilnasx)
#
# The NAS's docker builds the image over ssh from this checkout (native arm64, no registry) and
# tags it mint-server:local. /data/docker/mint/ gets docker-compose.yml (the previous one is kept
# as docker-compose.yml.bak); data/ and .env are created on the first run and never touched
# afterwards. The tunnel only starts once TUNNEL_TOKEN is set in .env; the mod mirror only
# downloads from mod.io once MINT_MODIO_TOKEN is.
set -euo pipefail

host=${1:-root@lilnasx}
dir=/data/docker/mint
repo=$(cd "$(dirname "$0")/.." && pwd)

DOCKER_HOST=ssh://$host docker build -f "$repo/server/Dockerfile" -t mint-server:local "$repo"

# the login shell on the NAS isn't bash; feed every remote script to bash explicitly
ssh "$host" bash -s <<REMOTE
set -euo pipefail
mkdir -p $dir
cd $dir
if [[ ! -d data ]]; then
	mkdir data
	chown 10001 data # the container's user
fi
if [[ ! -e .env ]]; then
	install -m 600 /dev/null .env
	printf 'TUNNEL_TOKEN=\nMINT_MODIO_TOKEN=\n' > .env
fi
REMOTE

scp -q "$repo/server/docker-compose.yml" "$host:$dir/docker-compose.yml.new"

ssh "$host" bash -s <<REMOTE
set -euo pipefail
cd $dir
if [[ -e docker-compose.yml ]] && ! cmp -s docker-compose.yml docker-compose.yml.new; then
	cp -p docker-compose.yml docker-compose.yml.bak
fi
mv docker-compose.yml.new docker-compose.yml
docker compose up -d mint-server
if grep -q '^TUNNEL_TOKEN=.' .env; then
	docker compose up -d
else
	echo "TUNNEL_TOKEN is empty in $dir/.env; the tunnel isn't started" >&2
fi
if ! grep -q '^MINT_MODIO_TOKEN=.' .env; then
	echo "MINT_MODIO_TOKEN is empty in $dir/.env; the mod mirror serves only uploaded files" >&2
fi
docker compose ps
REMOTE
