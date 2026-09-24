# proxy_tray

A Tauri v2 tray app that supervises the [`model-proxy-v3`](https://github.com/qidu/model_proxy_v3) SEA binary over
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

## How to build

The tray ships the proxy, so the proxy must be built first. Both are built from
source on their own platform — nothing is cross-compiled.

### Dependencies

| Tool | macOS | Windows |
| --- | --- | --- |
| Node.js | any modern Node for `npm ci` | same |
| Rust | `rustup` (host target) | `rustup default stable-x86_64-pc-windows-msvc` — MSVC, not GNU |
| C/C++ toolchain | Xcode Command Line Tools: `xcode-select --install` | Visual Studio Build Tools, "Desktop development with C++" (supplies `link.exe`) |
| Tauri CLI | `cargo install tauri-cli --version "^2"` (or `npx @tauri-apps/cli@^2`) | same |
| WebView2 | not needed (WKWebView) | preinstalled on Win 10/11, else the Evergreen bootstrapper |

The SEA build needs an **official** Node (nodejs.org, nvm,
`actions/setup-node`): the binary is a copy of the Node that built it, and
Homebrew's Node is a thin launcher with SEA compiled out. Both sequences below
therefore build under `npx --yes --package=node@22`, which fetches an official
Node without touching the system install.

```
npx --yes @tauri-apps/cli@^2 build
```

### macOS

```sh
# 1. Proxy: install deps and build the SEA binary.
cd model_proxy_v3
npm ci
npx --yes --package=node@22 node scripts/build-sea.js   # -> dist/model-proxy-v3-<host-triple>

# 2. Tray: stage the sidecar under the target-triple name externalBin needs.
cd ..
bash scripts/stage-sidecar.sh

# 3. Tray: build (or `cargo tauri dev` for a dev run).
cargo tauri build
```

### Windows

Run every step on Windows — SEA embeds a copy of the Node that built it, so a
`dist/model-proxy-v3-x86_64-apple-darwin` is useless here.

```
:: 1. Clone with the submodule (the file transport needs enabling; see Cloning).
git -c protocol.file.allow=always submodule update --init

:: 2. Proxy: install deps and build the SEA binary.
cd model_proxy_v3
npm ci
npx --yes --package=node@22 node scripts\build-sea.js   :: -> dist\model-proxy-v3-x86_64-pc-windows-msvc.exe

:: 3. Stage the sidecar by hand — build-sea.js already names the binary with
::    the host triple, so copy it to the externalBin location with the same name.
rustc --print host-tuple                                :: x86_64-pc-windows-msvc
copy model_proxy_v3\dist\model-proxy-v3-x86_64-pc-windows-msvc.exe ^
     src-tauri\binaries\model-proxy-v3-x86_64-pc-windows-msvc.exe

:: 4. Generate the Windows icon. Not optional: tauri-build writes the
::    executable's resource from src-tauri\icons\icon.ico and fails without it.
node scripts\make-icons.mjs

:: 5. From the project root (the directory holding src-tauri\), not src-tauri\.
cargo tauri build
```

Before step 5, apply the three `tauri.conf.json` edits under
[Config edits](#config-edits) below — the macOS defaults point at macOS paths
and the wrong icon set.

### The `externalBin` naming rule

`externalBin` entries are **untagged**, and Tauri appends `-<target-triple>`
itself (plus `.exe` on Windows) — unconditionally, with no existence check
(`tauri-utils/src/resources.rs`, `external_binaries`). So with an entry of
`binaries/model-proxy-v3` the file on disk must be
`binaries/model-proxy-v3-x86_64-apple-darwin`. Put the triple in the entry as
well and it is appended twice, and the build fails with
`resource path ...-x86_64-apple-darwin-x86_64-apple-darwin doesn't exist`.

So the entry is the **full path with the bare basename**: keep the `binaries/`
directory, and leave the file named `model-proxy-v3` — no target-triple suffix,
i.e. no arch-system-platform (`-x86_64-apple-darwin`,
`-x86_64-pc-windows-msvc`). Tauri supplies the host's triple itself.

Get the triple with `rustc --print host-tuple`. `scripts/build-sea.js` now emits
the triple name directly (e.g. `model-proxy-v3-x86_64-apple-darwin`), so no
rename is needed — `stage-sidecar.sh` copies it under the same name.

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
| `PROXY_CONFIG_PATH` | env if set, else `plugins."proxy-tray".configPath` in `src-tauri/tauri.conf.json`, else `~/.config/model-proxy-v3/proxy_config.toml` |
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
path that does not exist.

With neither `PROXY_CONFIG_PATH` nor `configPath` set, the tray uses
`~/.config/model-proxy-v3/proxy_config.toml` — the same literal on every
platform, matching `HOME_PROXY_CONFIG_PATH` in
`model_proxy_v3/src/utils/config-loader.ts`, so the tray and a directly launched
proxy agree on one file. The tray creates that directory at startup: it passes
`PROXY_CONFIG_PATH` explicitly, so the proxy's own resolver — which would
otherwise create it — never runs, and `persistProxyConfigToPath` writes
`<path>.tmp` with no `mkdir`. To point the tray at a different config, set
`configPath` in `tauri.conf.json` and rebuild, or set `PROXY_CONFIG_PATH`.

The resolved absolute path is shown in the window, so the live config is never
ambiguous. When no path is resolved (the home directory is undeterminable, so
`PROXY_CONFIG_PATH` is omitted), the window instead shows the two places the
proxy's own resolver will look — the working directory it inherits and the home
path — so there is always somewhere to drop a config.

Note the submodule clone does not carry `proxy_config.toml` (it is untracked
upstream) — copy it in after a fresh `git submodule update --init`.

## Cloning

`.gitmodules` points at a local relative path, so a fresh clone needs the `file`
transport explicitly enabled:

```sh
git -c protocol.file.allow=always submodule update --init
```

## Windows

Run the whole sequence under [How to build](#how-to-build) on Windows — SEA
embeds a copy of the Node that built it, so it **cannot cross-compile**. What
follows is Windows-specific.

- **The icon is mandatory.** `tauri-build` writes the executable's Windows
  resource from `src-tauri\icons\icon.ico` and fails with
  `icons/icon.ico not found` without it; `scripts\make-icons.mjs` writes it
  (16/32/48/256, the same ring as `icon.png`). The macOS build does not use the
  `.ico`.
- **Run `cargo tauri` from the project root** — the directory that holds
  `src-tauri/`, not `src-tauri/` itself: `cargo tauri` locates `src-tauri` by
  searching the cwd and then its children, never its parents.
- **`build-sea.js` quotes for cmd.exe** (`quoteForCmd`), so `npx.cmd`, the
  esbuild banner and paths with spaces all survive.

### Config edits

Two values in `src-tauri\tauri.conf.json` are macOS-specific and must change:

| Key | Now (macOS) | Windows |
| --- | --- | --- |
| `bundle.externalBin` | an absolute macOS path | `["binaries/model-proxy-v3"]` — untagged and relative; Tauri appends `-<triple>.exe` |
| `bundle.icon` | `["icons/icon.png"]` | add `"icons/icon.ico"` |

`plugins."proxy-tray".configPath` needs no per-platform value: it is absent by
default and the tray falls back to `~/.config/model-proxy-v3/proxy_config.toml`,
which is the same on every platform.

`externalBin` is relative on every platform: Tauri resolves it against
`src-tauri` and then appends the triple. An absolute path, or one that already
carries the triple, is wrong (see the naming rule above).

### Runtime caveats

The SEA build excludes `@github/keytar`, so `store_key_in_system = true` falls
back to the in-binary body store, which keeps keys in plaintext inside the
executable and needs a writable directory. `sdk://` routes are also unsupported
in the binary — the `chatjimmy` submodule is excluded (see `build-sea.js`).

A start also ensures a Windows Defender inbound allow rule for the bundled
`model-proxy-v3.exe` (`src-tauri/src/firewall.rs`), so a tray-launched proxy is
reachable from other hosts. The rule is program-scoped (any port and protocol)
and named `Model Proxy Inbound`. Adding a rule needs Administrator and the tray
is not elevated, so this is best-effort: the outcome is mirrored to the window's
LOG section and the proxy starts regardless. Without elevation the rule is not
added — run the tray once as Administrator, or add the rule by hand.
