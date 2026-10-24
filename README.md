# proxy_tray

A Tauri v2 tray app that supervises the [`model-proxy-v3`](https://github.com/qidu/model_proxy_v3) SEA binary over
JSON-RPC 2.0 on stdio. It never proxies model traffic — clients keep talking
HTTP to the proxy on port 8788.

The design this implements is `docs/architecture/design_tauri_tray.md` in the
`submodules/model_proxy_v3` submodule.

## Layout

| Path | What |
| --- | --- |
| `submodules/model_proxy_v3/` | git submodule — the proxy this app supervises |
| `ui/` | the window's frontend (`frontendDist`), vanilla HTML/JS, no bundler |
| `scripts/stage-sidecar.sh` | copies the built SEA binary (path given as its argument) into `src-tauri/binaries/` |
| `scripts/make-icons.mjs` | regenerates `src-tauri/icons/*.png` and `icon.ico` |
| `src-tauri/` | the Rust core: tray, supervisor, JSON-RPC client |
| `.github/workflows/ci.yml` | builds the sidecar, lints, tests, and bundles on macOS and Windows (see [CI](#ci)) |

## How to build

The tray ships the proxy, so the proxy must be built first. Both are built from
source on their own platform — nothing is cross-compiled.

The build runs in four steps, and the first is installing the toolchain (Rust
via [rustup](https://rust-lang.org/tools/install/), the C/C++ toolchain, and
Node.js — see [Dependencies](#dependencies)):

1. Install Rust (`rustc` + `cargo`), Node.js, and the C/C++ toolchain.
2. Build the proxy SEA binary.
3. Stage the sidecar into `src-tauri/binaries/`.
4. Build the tray.

### Dependencies

| Tool | macOS | Windows |
| --- | --- | --- |
| Node.js | any modern Node (`engines` says `>=19`; the SEA build itself uses `node@22`) | same |
| Rust | [rustup](https://rust-lang.org/tools/install/) (host target) | `rustup default stable-x86_64-pc-windows-msvc` — MSVC, not GNU |
| C/C++ toolchain | Xcode Command Line Tools: `xcode-select --install` | Visual Studio Build Tools, "Desktop development with C++" (supplies `link.exe`) |
| Tauri CLI | `cargo install tauri-cli --locked` or `cargo install tauri-cli --version "^2"` (or `npx @tauri-apps/cli@^2`) | same |
| WebView2 | not needed (WKWebView) | preinstalled on Win 10/11, else the Evergreen bootstrapper |

Key locked crate versions (see `src-tauri/Cargo.lock`; `cargo update` refreshes them):

| Crate | Locked at | Note |
| --- | --- | --- |
| `tauri` | 2.12.4 | pulls `tray-icon`, `wry`, `tao` |
| `tray-icon` | 0.25.1 | **must stay ≥ 0.25.1**: 0.24.x never receives left clicks on macOS 27, so the tray click that opens the window silently does nothing |
| `wry` / `tao` | 0.57.0 / 0.37.1 | WebView / windowing, pulled by `tauri` |

The SEA build needs an **official** Node (nodejs.org, nvm,
`actions/setup-node`): the binary is a copy of the Node that built it, and
Homebrew's Node is a thin launcher with SEA compiled out. Both sequences below
therefore build under `npx --yes --package=node@22`, which fetches an official
Node without touching the system install.

No lockfile is committed in the submodule at the pinned commit, so `npm ci`
fails there — use `npm install --omit=optional --ignore-scripts`, which is what
the submodule's own `Dockerfile` does. `--omit=optional` skips the
`@github/keytar` git-URL dependency (the SEA excludes it anyway) and
`--ignore-scripts` its `prebuild-install`. No production dependency needs an
install script.

### macOS

```sh
# 1. Proxy: install deps and build the SEA binary.
cd submodules/model_proxy_v3
npm install --omit=optional --ignore-scripts
npm run build:native   # = npx --yes --package=node@22 node scripts/build-sea.js
                       # -> dist/model-proxy-v3-<host-triple>

# 2. Tray: stage the sidecar under the target-triple name externalBin needs.
cd ../..
bash scripts/stage-sidecar.sh \
  "submodules/model_proxy_v3/dist/model-proxy-v3-$(rustc --print host-tuple)"
#    The path is not optional — the script's own fallbacks look for a sibling
#    model_proxy_v3/, never submodules/model_proxy_v3/ (see below).
#    Or by hand (Apple Silicon):
#    mkdir -p src-tauri/binaries
#    cp submodules/model_proxy_v3/dist/model-proxy-v3-aarch64-apple-darwin src-tauri/binaries/

# 3. Tray: build (or `cargo tauri dev` for a dev run).
cargo tauri build
```

### Windows

Run every step on Windows — SEA embeds a copy of the Node that built it, so a
`dist/model-proxy-v3-x86_64-apple-darwin` is useless here.

```
:: 1. Clone with the submodule (see Cloning).
git submodule update --init

:: 2. Proxy: install deps and build the SEA binary.
::    npm run build:native = npx --yes --package=node@22 node scripts\build-sea.js
::    -> dist\model-proxy-v3-x86_64-pc-windows-msvc.exe
cd submodules\model_proxy_v3
npm install --omit=optional --ignore-scripts
npm run build:native
cd ..\..

:: 3. Stage the sidecar by hand — build-sea.js already names the binary with the
::    host triple, so copy it to the externalBin location under the same name.
::    (scripts\stage-sidecar.sh works too, but only under bash, and it writes the
::    destination without the .exe that tauri-utils appends on Windows.)
::    `rustc --print host-tuple` should print x86_64-pc-windows-msvc.
rustc --print host-tuple
mkdir src-tauri\binaries 2>nul
copy submodules\model_proxy_v3\dist\model-proxy-v3-x86_64-pc-windows-msvc.exe ^
     src-tauri\binaries\model-proxy-v3-x86_64-pc-windows-msvc.exe

:: 4. From the project root (the directory holding src-tauri\), not src-tauri\.
cargo tauri build
```

No `tauri.conf.json` edit is needed — the committed config is already
cross-platform (see [Config edits](#config-edits)).

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

Get the triple with `rustc --print host-tuple`. `scripts/build-sea.js` emits the
triple name directly (e.g. `model-proxy-v3-x86_64-apple-darwin`), so no rename
is needed on macOS — `stage-sidecar.sh` copies it under the same name. On
Windows it does need the `.exe` that `build-sea.js` emits and Tauri looks for;
the script writes its destination without one, so copy that file by hand (the
sequence above does).

Once copied into the build, the triple is stripped again: the sidecar is
`target/{debug,release}/model-proxy-v3`, and inside the bundle
`Proxy-tray.app/Contents/MacOS/model-proxy-v3` (`productName` is `Proxy-tray`).

`cargo check` **requires** the staged sidecar: `tauri_build::build()` copies
`externalBin` during `build.rs` and errors on a missing file.

### Where the staged sidecar comes from

`scripts/stage-sidecar.sh` takes the built SEA binary as its only argument.
Pass it explicitly: the script's no-argument fallbacks look for
`model_proxy_v3/dist/` and `../model_proxy_v3/dist/`, and the submodule lives at
`submodules/model_proxy_v3/dist/`, so a bare invocation finds nothing and exits
1. A fresh submodule clone also has an empty `dist/` until you build there, so
the staged binary is often a sibling checkout's instead. Either is fine as long
as it includes `--rpc`; check with:

```sh
printf '{"jsonrpc":"2.0","method":"status.get","params":{},"id":1}\n' \
  | PORT=18788 PROXY_CONFIG_PATH="$PWD/submodules/model_proxy_v3/proxy_config.toml" \
    ./src-tauri/binaries/model-proxy-v3-x86_64-apple-darwin --rpc
```

A silent control channel means the binary predates `--rpc`. The submodule source
has carried it since `fe3635b` (`src/rpc.ts`), and the pinned commit
`c2f4dcb8` (`v2.8.1-1-gc2f4dcb`) is 68 commits past that on
`origin/feature/targeting_failover`, so a SEA rebuilt from the submodule is fine.

`.gitmodules` pins `branch = feature/targeting_failover`, so `git submodule
update --remote` follows that branch. Without the pin it would follow
`origin/HEAD`, and if that ever moved to `main`, `--remote` could check out a
commit without `--rpc` and the tray would lose its control channel.

## CI

`.github/workflows/ci.yml` runs on push to `main`, on pull requests, on
`workflow_dispatch`, and on `release: published` — the last one is the hook for
the web UI's **Create a new release** button, so publishing a release there
builds the installers and attaches them without anything pushed by hand.

Each runner builds its own sidecar, because a SEA binary is a copy of the Node
that produced it and cannot be cross-compiled. `macos-latest` is arm64, so the
macOS artifacts are arm64-only; an Intel or universal `.dmg` needs an Intel
runner.

Per platform, the job:

1. Rewrites `git@github.com:` to `https://github.com/` globally, before checkout:
   `.gitmodules` declares the submodule over SSH and the runner has no key. The
   repository is public, so no secret is involved.
2. Checks out **without** letting `actions/checkout` touch submodules, then runs
   `git submodule update --init submodules/model_proxy_v3` itself, non-recursively
   (see [Cloning](#cloning)). checkout's own submodule handling is recursive in
   places it cannot be told not to be, and it is not needed here.
3. Runs `npm install --omit=optional --ignore-scripts` and `npm run build:native`
   in the submodule. `npm ci` is impossible there: no lockfile is committed.
4. Stages the sidecar, and on Windows adds the `.exe` suffix `stage-sidecar.sh`
   omits.
5. Runs `cargo fmt --all --check` (macOS only — formatting is a property of the
   source), `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
6. On every event but a pull request, runs `tauri build` and uploads the `.dmg` /
   `.msi` / `-setup.exe` as a workflow artifact; on a published release it also
   attaches them to that release.

The `.app` is uploaded only inside the `.dmg`: `upload-artifact` preserves
neither permissions nor symlinks, so an extracted `.app` comes back broken.

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

## UI Features

### Export Buttons
The window provides three export actions that invoke the proxy's CLI:
- **Export for Pi** — `--export-pi-models`
- **Export for OpenClaw** — `--export-openclaw-providers`
- **Export for DSH** — `--export-dsh`

### Model Testing
The model selector supports **multi-select** (hold Ctrl/Cmd to choose multiple);
selected options are marked with a **✓**. Clicking **Test now** starts a
**pinned** run: it runs `model.test` sequentially for each selected model —
showing per-model results with latency and token usage, then a summary — and
repeats every **300s**. Clicking **Test now** again stops the repeat. The
**Pin 300s** checkbox reflects whether the pinned run is active; clicking it
does not start testing.

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

## Config field aliases

The proxy's hand-rolled TOML parser (`model_proxy_v3/src/utils/config-loader.ts`)
accepts both canonical and short field names in inline tables. Canonical names
win when both are present.

### `[models.*]` inline tables (per-model overrides)

```toml
[models.openai]
gpt-4o = {target = "openai", base_url = "https://api.openai.com", api_key = "sk-...", upstream_mode = "openai-completions"}
```

| Canonical | Short alias | Notes |
| --- | --- | --- |
| `base_url` | `base` | |
| `upstream_mode` | `mode` | Must be a value from `UPSTREAM_MODES` |
| `api_key` | `key` | |
| `target` | — | Optional; defaults to the model key (`gpt-4o`) |

`url` is **not** accepted — use `base_url` or `base`.

`transforms` (a comma-separated transform list) and `max_tokens` (a bare number)
are also read from inline tables; both are optional.

### `[models.passthrough]` inline tables (passthrough targets)

```toml
[models.passthrough]
openai = {base = "https://api.openai.com", key = "sk-...", mode = "openai-completions", share = 1}
```

A bare `[passthrough]` is **not** a section — it is an unknown header, so a
config still using one has its targets silently ignored (the loader warns about
an unknown section). `[models.passthrough.<name>]` sub-sections are unsupported
too: every target must be an inline table directly under
`[models.passthrough]`.

| Canonical | Short alias | Notes |
| --- | --- | --- |
| `base` | — | **Canonical here** (short name is primary) |
| `base_url` | `url` | Long aliases accepted |
| `mode` | — | **Canonical here** |
| `upstream_mode` | — | Long alias accepted |
| `key` | — | **Canonical here** |
| `api_key` | — | Long alias accepted |
| `target` | — | **Not a field** — the target name is the TOML key (`openai`) |

`share` and `timeout` are numeric-only (no quotes).

### `[models.*]` section-level keys (category defaults)

```toml
[models.openai]
upstream_mode = "openai-completions"
base_url = "https://api.openai.com"
api_key = "sk-..."
```

Only canonical names accepted here — short names (`mode`, `base`, `key`) are
rejected with an error surfaced in the TUI/dashboard.

### Array format (legacy, still supported)

```toml
[models.openai]
gpt-4o = ["target", "base_url", "api_key", "upstream_mode"]
```

Purely positional — no field names. Accepted lengths are 1, 3, 4, 5 and 6; a
2-element array is rejected, and so is a 7th element. The positions are:

| Index | Field |
| --- | --- |
| 0 | `target` |
| 1 | `base_url` |
| 2 | `api_key` |
| 3 | `mode` |
| 4 | `transforms` (CSV) |
| 5 | `max_tokens` (bare number) |

So `[target]`, `[target, base_url, api_key]`, then optionally `mode`, then
`transforms`, then `max_tokens`, in that order. `base` and `url` are not valid in
array form.

## Cloning

`.gitmodules` declares the submodule with an SSH URL
(`git@github.com:qidu/model_proxy_v3.git`) — no local path, so the `file`
transport is not involved. Initializing it therefore needs GitHub SSH auth; both
repositories are public, so a redirect to HTTPS is the fallback:

```sh
# fresh clone — recursion off; put `-c url."https://github.com/".insteadOf="git@github.com:"`
# before `clone` if you have no SSH key
git clone --no-recurse-submodules https://github.com/qidu/proxy_tray.git
git -C proxy_tray submodule update --init

# existing clone
git submodule update --init                                                            # with an SSH key
git -c url."https://github.com/".insteadOf="git@github.com:" submodule update --init    # without
```

`git submodule update --init` is already non-recursive — `--recursive` is the flag
that turns recursion *on*, and it is deliberately absent everywhere here. The same
goes for `git clone`: plain `git clone` leaves the submodule alone, and
`--recurse-submodules` is what would recurse. `model_proxy_v3` has five nested
submodules, but nothing built here imports them: `build-sea.js` marks exactly one
external (`@github/keytar`), and every `submodules/` mention under `src/` is a
comment.

### Wanting one of `model_proxy_v3`'s own submodules

Nothing above needs them — the sidecar builds fine with all five uninitialized.
If you want one anyway (`submodules/kompress`, `submodules/privacy-filter`, …),
clone that repository **standalone** — a fresh checkout with no stale gitlink to
trip over — and build its binary there, then hand the result to the staging step:

```sh
git clone https://github.com/qidu/model_proxy_v3.git   # or the git@ URL with a key
cd model_proxy_v3
git submodule update --init submodules/<name>          # name the path — see below
npm install --omit=optional --ignore-scripts
npm run build:native                                   # -> dist/model-proxy-v3-<triple>

cd /path/to/proxy_tray
bash scripts/stage-sidecar.sh "$PWD/../model_proxy_v3/dist/model-proxy-v3-$(rustc --print host-tuple)"
```

Two things that will otherwise cost you a `fatal`:

- **Name the submodule path.** A bare `git submodule update --init` inside
  `model_proxy_v3` aborts — `fatal: No url found for submodule path … in
  .gitmodules` — because it enumerates *every* gitlink in the tree, and one of
  them has no `.gitmodules` entry. Passing a path skips the enumeration, so
  exactly the submodule you named is initialized. `--recursive` fails the same
  way, for the same reason.
- **All five of its submodules are declared over SSH**, so without a key add the
  same redirect used above:
  `git -c url."https://github.com/".insteadOf="git@github.com:" submodule update --init submodules/<name>`.

`build:native` is host-only — SEA embeds the Node that runs it — so the binary
suits the machine that built it and no other, same as the platform sequences
above. On Windows `stage-sidecar.sh` writes the destination without the `.exe`
Tauri appends, so copy the file by hand instead (see [How to build](#how-to-build)).

## Windows

Run the whole sequence under [How to build](#how-to-build) on Windows — SEA
embeds a copy of the Node that built it, so it **cannot cross-compile**. What
follows is Windows-specific.

- **The icon is committed.** `src-tauri\icons\icon.ico` is tracked and listed in
  `bundle.icon`, so a fresh clone builds as-is — `tauri-build` writes the
  executable's Windows resource from it. Run `node scripts\make-icons.mjs` only
  after changing the ring art: it rewrites `icon.png`, the three tray PNGs, and
  the `.ico` (16/32/48/256, the same ring). The macOS build does not use the
  `.ico`.
- **Run `cargo tauri` from the project root** — the directory that holds
  `src-tauri/`, not `src-tauri/` itself: `cargo tauri` locates `src-tauri` by
  searching the cwd and then its children, never its parents.
- **`build-sea.js` quotes for cmd.exe** (`quoteForCmd`), so `npx.cmd`, the
  esbuild banner and paths with spaces all survive.

### Config edits

None are needed. `src-tauri\tauri.conf.json` is already cross-platform: the same
file builds on macOS and on Windows.

| Key | Value | Why it works on both |
| --- | --- | --- |
| `bundle.externalBin` | `["binaries/model-proxy-v3"]` | relative and untagged — Tauri appends `-<triple>`, plus `.exe` on Windows |
| `bundle.icon` | `["icons/icon.png", "icons/icon.ico"]` | Tauri picks whichever the platform's bundler wants |

`externalBin` must stay relative and untagged: Tauri resolves it against
`src-tauri` and then appends the triple. An absolute path, or one that already
carries the triple, is wrong (see the naming rule above).

`plugins."proxy-tray".configPath` needs no per-platform value: it is absent by
default and the tray falls back to `~/.config/model-proxy-v3/proxy_config.toml`,
which is the same on every platform.

### Runtime caveats

The SEA build excludes `@github/keytar`, so `store_key_in_system = true` falls
back to the in-binary body store, which keeps keys in plaintext inside the
executable and needs a writable directory. `sdk://` routes are also unsupported
in the binary — the optional integration behind them is excluded too, and both
fail soft at runtime (see `WHAT IS NOT IN THE BINARY` in `build-sea.js`).

A start also ensures a Windows Defender inbound allow rule for the bundled
`model-proxy-v3.exe` (`src-tauri/src/firewall.rs`), so a tray-launched proxy is
reachable from other hosts. The rule is program-scoped (any port and protocol)
and named `Model Proxy Inbound`. Adding a rule needs Administrator and the tray
is not elevated, so this is best-effort: the outcome is mirrored to the window's
LOG section and the proxy starts regardless. Without elevation the rule is not
added — run the tray once as Administrator, or add the rule by hand.
