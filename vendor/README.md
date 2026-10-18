# Vendored dependencies

Third-party repositories mint builds from, copied in so the build never fetches from someone
else's git repo. Each directory keeps its own workspace and license; the root workspace excludes
`vendor/`.

| Directory | Upstream | Revision |
|---|---|---|
| `modio-rs` | https://github.com/trumank/modio-rs (branch `dev`) | d979c0a1bf0fd865bb30feb850079530ec6b84ba |
| `patternsleuth` | https://github.com/trumank/patternsleuth | 8bcbd4371989f29eb7dfcb65e645a3bde23daf0d |
| `proxy_dll` | https://github.com/trumank/proxy_dll | a0c0772634ee751b580f95dd96c460885c8bdf2b |
| `repak` | https://github.com/trumank/repak | 355b5f62f51959c7cc6dd5a51708646ef483065d |
| `retour-rs` | https://github.com/Hpmason/retour-rs | 94be4740ec0c624f61a074f439e6351fefc0e697 |
| `uasset_utils` | https://github.com/trumank/uasset_utils | 70071ef27913bac269e4ddbcf732aff29d268f2b |
| `unrealmodding` | https://github.com/trumank/unrealmodding (branch `patches`) | 77475d83712a629a1e89a84eac4a3378ee5e21d4 |

## Local changes

Everything not listed here is byte-identical to the revision above (minus `.git`).

- `uasset_utils`: `unreal_asset` and `repak` point at the vendored copies instead of git.
