# mint_server

Hosts mod presets that mint shows as `[Remote] <name>`. Publishers upload a preset from mint with
a publish key; everyone else only reads.

## Deploy

```sh
server/deploy.sh                 # default host root@lilnasx
```

builds the image with the NAS's docker over ssh (native arm64, tagged `mint-server:local`, no
registry) and starts it from `/data/docker/mint/`:

```
docker-compose.yml   copied from this directory on every deploy (previous one: .bak)
.env                 TUNNEL_TOKEN=…; created empty on the first deploy, never overwritten
data/                presets and publish keys, owned by uid 10001 (the container's user)
```

Both containers sit in the project's own `mint` network; nothing is published on the host.
The tunnel only starts once `TUNNEL_TOKEN` is set.

### Tunnel (once)

1. Cloudflare Zero Trust → Networks → Tunnels → create a cloudflared tunnel, choose Docker, copy
   the token into `/data/docker/mint/.env` as `TUNNEL_TOKEN=<token>`.
2. Public hostname: `mint.kohlroulade.lgbt`, service `HTTP`, URL `mint-server:8080`.
3. `cd /data/docker/mint && docker compose up -d`, or deploy again.

Check: `curl https://mint.kohlroulade.lgbt/health` prints `ok`.

## Publish keys

On the NAS:

```sh
cd /data/docker/mint
docker compose run --rm --no-deps mint-server mint_server gen-key alex
```

prints a key (shown once; it goes into mint's Settings → Publish key) and a line like
`alex 3f2a…` to append to `data/keys`. The file stores only SHA-256 hashes of keys, so it can't
be used to publish. It is re-read on every publish: adding or removing a line takes effect
without a restart. Without a `data/keys` file nobody can publish.

## Data

```
data/keys                            publish keys (hashes)
data/presets/<slug>/<revision>.json  every published revision, never modified
data/deleted/<slug>-<unix time>/     unpublished presets, kept
```

Back up `data/`. To roll back a preset, publish an older revision's content again: it becomes the
newest revision.

## Local

```sh
MINT_SERVER_DATA=./data MINT_SERVER_LISTEN=127.0.0.1:8080 cargo run -p mint_server
```

## API

| Request | Auth | |
|---|---|---|
| `GET /api/v1/presets` | | all presets, latest revision each |
| `GET /api/v1/presets/{slug}` | | latest revision |
| `GET /api/v1/presets/{slug}/{revision}` | | one revision, cacheable forever |
| `PUT /api/v1/presets` | `Bearer <key>` | publish `{"name": …, "mods": […]}`; same name = new revision, identical content = no new revision |
| `DELETE /api/v1/presets/{slug}` | `Bearer <key>` | unpublish |
| `GET /health` | | `ok` |

The format is defined in `remote_preset/`. Uploads are limited to 512 KiB and 2000 mods and are
validated before anything is stored.
