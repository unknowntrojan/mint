# mint_server

Hosts mod presets that mint shows as `[Remote] <name>`. Publishers upload a preset from mint with
a publish key; everyone else only reads.

## Run

On the host, from this directory:

```sh
mkdir -p data && sudo chown 10001 data     # the container runs as uid 10001
docker compose up -d --build
```

It listens on `127.0.0.1:8087` only. Point the public hostname (e.g. `mint.kohlroulade.lgbt`) at
`http://127.0.0.1:8087` in the Cloudflare tunnel or reverse proxy. TLS ends there; the server
itself speaks plain HTTP.

## Publish keys

```sh
docker compose run --rm mint-server mint_server gen-key alex
```

prints a key (shown once; it goes into mint's settings) and a line like `alex 3f2a…` to append to
`data/keys`. The file stores only SHA-256 hashes of keys, so it can't be used to publish. It is
re-read on every publish: adding or removing a line takes effect without a restart. Without a
`data/keys` file nobody can publish.

## Data

```
data/keys                          publish keys (hashes)
data/presets/<slug>/<revision>.json  every published revision, never modified
data/deleted/<slug>-<unix time>/   unpublished presets, kept
```

Back up `data/`. To roll back a preset, publish an older revision's content again: it becomes the
newest revision.

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
