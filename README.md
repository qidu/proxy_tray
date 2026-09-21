# proxy_tray

A Tauri v2 tray app that supervises the `model-proxy-v3` SEA binary over
JSON-RPC 2.0 on stdio. It never proxies model traffic — clients keep talking
HTTP to the proxy on port 8788.

The design this implements is `docs/design_tauri_tray.md` in the
`submodules/model_proxy_v3` submodule.

## Layout

| Path | What |
| --- | --- |
| `submodules/model_proxy_v3/` | git submodule — the proxy this app supervises |
| `ui/` | the window's frontend (`frontendDist`), vanilla HTML/JS, no bundler |
| `scripts/stage-sidecar.sh` | copies the built SEA binary into `src-tauri/binaries/` |
| `scripts/make-icons.mjs` | regenerates `src-tauri/icons/*.png` |
| `src-tauri/` | the Rust core: tray, supervisor, JSON-RPC client |

## Build order

The tray ships the proxy, so the proxy must be built first.

1. Build the SEA binary inside the submodule:

   ```sh
   cd model_proxy_v3
   npm ci
   npx --yes --package=node@26 node scripts/build-sea.js
   ```

   Homebrew's Node cannot build a SEA ("SEA support is compiled out"), hence the
   `npx --package=node@26` wrapper.

2. Stage it under the target-triple name `externalBin` requires:

   ```sh
   bash scripts/stage-sidecar.sh
   ```

3. `cargo check` (or `tauri dev` / `tauri build`, which need the Tauri CLI).

### The `externalBin` naming rule

`externalBin` entries are **untagged**, and Tauri appends `-<target-triple>`
itself (plus `.exe` on Windows) — unconditionally, with no existence check
(`tauri-utils/src/resources.rs`, `external_binaries`). So with an entry of
`binaries/model-proxy-v3` the file on disk must be
`binaries/model-proxy-v3-x86_64-apple-darwin`. Put the triple in the entry as
well and it is appended twice, and the build fails with
`resource path ...-x86_64-apple-darwin-x86_64-apple-darwin doesn't exist`.

Get the triple with `rustc --print host-tuple`. `scripts/build-sea.js` emits a
platform-tagged name (`model-proxy-v3-macos-x64`), so a rename into the triple
form is always needed.

Once copied into the build, the triple is stripped again: the sidecar is
`target/{debug,release}/model-proxy-v3`, and inside the bundle
`proxy-tray.app/Contents/MacOS/model-proxy-v3`.

`cargo check` **requires** the staged sidecar: `tauri_build::build()` copies
`externalBin` during `build.rs` and errors on a missing file.

### Where the staged sidecar comes from

`scripts/stage-sidecar.sh` prefers the submodule's own `dist/`, then falls back
to the sibling checkout's. The submodule is a fresh clone whose `dist/` is
empty, so the staged binary is normally the sibling's. It must include `--rpc`;
check with:

```sh
printf '{"jsonrpc":"2.0","method":"status.get","params":{},"id":1}\n' \
  | PORT=18788 PROXY_CONFIG_PATH="$PWD/model_proxy_v3/proxy_config.toml" \
    ./src-tauri/binaries/model-proxy-v3-x86_64-apple-darwin --rpc
```

A silent control channel means the binary predates `--rpc`. The submodule
**source** now carries it (`src/rpc.ts`, commit `fe3635b`), so a SEA rebuilt
from the submodule is fine — but that commit sits on
`origin/feature/targeting_failover`, not `origin/main`:

```sh
git -C model_proxy_v3 merge-base --is-ancestor fe3635b origin/main   # false today
```

`.gitmodules` pins `branch = feature/targeting_failover`, so `git submodule
update --remote` follows that branch and lands on `fe3635b`. Without the pin it
would follow `origin/HEAD`, and if that ever moved to `main`, `--remote` would
check out a commit without `--rpc` and the tray would lose its control channel.

## Configuration

The app resolves the config file and port as:

| Setting | Source |
| --- | --- |
| `PROXY_CONFIG_PATH` | env if set, else `plugins."proxy-tray".configPath` in `src-tauri/tauri.conf.json` |
| `PORT` | env if set, else `plugins."proxy-tray".port` in `src-tauri/tauri.conf.json`, else `8788` |

Both fall back to the same `plugins."proxy-tray"` block, because a
Finder-launched `.app` inherits no environment at all — env-only would leave it
stuck on the defaults. The tray reads the block once at startup and forwards
both values to the sidecar, so the window and the proxy can never disagree about
which port or config is live.

The proxy has **no** port setting of its own: it reads `PORT` from the
environment (`server.ts:13`), so the tray's value is the only knob. A
non-numeric or out-of-range `PORT`, or an out-of-range `port` in the conf, is
reported on stderr and the default is used.

The config path is **absolute and never derived from the working directory**.
A Finder-launched `.app` has `cwd` `/`, where any relative default resolves to a
path that does not exist — and the proxy's own fallback (`./proxy_config.toml`,
`server.ts:45`) is cwd-relative too, so it would quietly read the wrong file.
To point the tray at a different config, edit `configPath` in `tauri.conf.json`
and rebuild, or set `PROXY_CONFIG_PATH`.

If neither source names a config, the tray does **not** pass the variable at
all: an empty `PROXY_CONFIG_PATH` is falsy and `server.ts:45`'s `||` would
silently swap in `./proxy_config.toml`. The window shows `not configured —
set PROXY_CONFIG_PATH` instead.

The resolved absolute path is shown in the window, so the live config is never
ambiguous.

Note the submodule clone does not carry `proxy_config.toml` (it is untracked
upstream) — copy it in after a fresh `git submodule update --init`.

## Cloning

`.gitmodules` points at a local relative path, so a fresh clone needs the `file`
transport explicitly enabled:

```sh
git -c protocol.file.allow=always submodule update --init
```

## Windows

The SEA build excludes `@github/keytar`, so `store_key_in_system = true` falls
back to the in-binary body store, which keeps keys in plaintext inside the
executable and needs a writable directory.
