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
- `repak/oodle_loader`: never downloads the Oodle library (it must sit next to the executable;
  the friendpack installers put it there). Drops `ureq`, `sha2`, `hex`; `libloading` 0.9 -> 0.8
  to share the copy eframe uses.
- `modio-rs`: reqwest 0.11 -> 0.12, reqwest-middleware 0.2 -> 0.4 (+ `multipart`), http 0.2 -> 1,
  bitflags 1 -> 2 (explicit derives, `.bits` -> `.bits()`), serde_with 2 -> 3. Gets mint off
  hyper 0.14 / rustls 0.21, which no longer receive updates.
- `unrealmodding`: num_enum 0.6 -> 0.7, ordered-float 3 -> 4, thiserror 1 -> 2, rustc-hash 1 -> 2.
  Tests and doc examples updated for the `skip_data` argument the `patches` branch added to
  `Asset::new` (they didn't compile upstream).
- `uasset_utils`: typed-path 0.8 -> 0.12 (same as zip).
- `patternsleuth`: itertools 0.12 -> 0.15, strum 0.25 -> 0.27, windows 0.52 -> 0.61, object 0.32 -> 0.40
  (section flag checks adapted to its newtype flags). Hook resolvers verified to return identical
  results against FSD-Win64-Shipping.exe before/after.
- `proxy_dll`: windows-sys 0.59 -> 0.61 (`BOOL` moved to `windows_sys::core`). Hook DLL exports
  unchanged.
