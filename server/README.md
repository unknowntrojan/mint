# mint_server

Hosts mod presets that mint shows as `[Remote] <name>`, and mint releases that release builds
update themselves to. Publishers upload with a publish key; everyone else only reads.

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

## Releases

```sh
./release.sh "what changed"        # from the repo root, on AlexPC
```

asks the server for the next release number, builds Linux + Windows (`--profile dist`) with that
number baked in (`MINT_RELEASE`), uploads both and publishes a manifest signed with the release
key, then tags the commit `release-<n>`. Builds made this way check the server on start and
replace themselves with a newer release; builds without `MINT_RELEASE` never update.

- **Signing key:** `~/.local/share/mint-release/signing.key`, made once with
  `mint_release keygen`. Its public half is `PUBLIC_KEY` in `remote_preset/src/release.rs`,
  compiled into mint and the server; both refuse manifests it didn't sign. **Back the key up.**
  Without it no installed mint accepts updates any more, and a new key only reaches friends
  through a new installer.
- **Publish key:** the same one presets use (mint's setting, or `MINT_PUBLISH_KEY`).
- **Rollback:** publish the old commit as a new release (`git checkout release-<n>`, then
  `./release.sh`). mint never installs a lower number.
- **Failed updates:** mint remembers the release it tried last (`update-attempt` in its cache
  directory) and doesn't retry it automatically; the next release, or a new installer, fixes it.
- `friendpack/pack.sh` puts the latest release into the installer zip, so new friends start on a
  build that updates itself.

## Data

```
data/keys                            publish keys (hashes)
data/presets/<slug>/<revision>.json  every published revision, never modified
data/deleted/<slug>-<unix time>/     unpublished presets, kept
data/releases/<n>/                   binaries of release n, plus manifest.json once published
```

Releases take ~50 MB each; delete old `data/releases/<n>/` directories by hand if space matters
(never the latest).

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
| `GET /api/v1/releases/latest` | | signed manifest of the latest release |
| `PUT /api/v1/releases/{n}/files/{name}` | `Bearer <key>` | upload a binary of unpublished release n (≤ 95 MB, streamed) |
| `PUT /api/v1/releases/{n}` | `Bearer <key>` | publish release n: the signed manifest; its files must be uploaded |
| `GET /api/v1/releases/{n}/files/{name}` | | a binary of a published release, cacheable forever |
| `GET /health` | | `ok` |

The formats are defined in `remote_preset/`. Preset uploads are limited to 512 KiB and 2000 mods
and are validated before anything is stored.
