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
| `scripts/stage-sidecar.sh` | downloads the latest host-specific proxy release into `src-tauri/binaries/` |
| `scripts/make-icons.mjs` | regenerates `src-tauri/icons/*.png` and `icon.ico` |
| `src-tauri/` | the Rust core: tray, supervisor, JSON-RPC client |
| `.github/workflows/ci.yml` | downloads the sidecar, lints, tests, and bundles on macOS and Windows (see [CI](#ci)) |

## How to build

The tray ships the proxy, so the host-specific proxy release must be staged
first. The staging script downloads the latest published release; the proxy
submodule and its Node.js build toolchain are not needed for a normal tray build.

1. Install Rust (`rustc` + `cargo`), the C/C++ toolchain, Bash, and `curl`.
2. Run `bash scripts/stage-sidecar.sh` to stage the sidecar in `src-tauri/binaries/`.
3. Build the tray.

### Dependencies

| Tool | macOS | Windows |
| --- | --- | --- |
| Node.js | only for the `npx` Tauri CLI option or icon generation | same |
| Rust | [rustup](https://rust-lang.org/tools/install/) (host target) | `rustup default stable-x86_64-pc-windows-msvc` — MSVC, not GNU |
| C/C++ toolchain | Xcode Command Line Tools: `xcode-select --install` | Visual Studio Build Tools, "Desktop development with C++" (supplies `link.exe`) |
| Download tools | Bash and `curl` | Git Bash, `curl`, and Windows `tar` for the release ZIP |
| Tauri CLI | `cargo install tauri-cli --locked` or `cargo install tauri-cli --version "^2"` (or `npx @tauri-apps/cli@^2`) | same |
| WebView2 | not needed (WKWebView) | preinstalled on Win 10/11, else the Evergreen bootstrapper |

Key locked crate versions (see `src-tauri/Cargo.lock`; `cargo update` refreshes them):

| Crate | Locked at | Note |
| --- | --- | --- |
| `tauri` | 2.12.4 | pulls `tray-icon`, `wry`, `tao` |
| `tray-icon` | 0.25.1 | **must stay ≥ 0.25.1**: 0.24.x never receives left clicks on macOS 27, so the tray click that opens the window silently does nothing |
| `wry` / `tao` | 0.57.0 / 0.37.1 | WebView / windowing, pulled by `tauri` |

### macOS

```sh
# 1. Download the latest proxy release for this Mac and stage it for externalBin.
bash scripts/stage-sidecar.sh

# 2. Build (or `cargo tauri dev` for a dev run).
cargo tauri build
```

### Windows

Run the staging script from Git Bash. It selects the Windows x64 ZIP on the
standard MSVC host, extracts the executable, `keytar.node`, and keytar's license,
and stages all three files:

```sh
bash scripts/stage-sidecar.sh
cargo tauri build
```

`rustc --print host-tuple` should print `x86_64-pc-windows-msvc`. Run the Tauri
build from the project root with the MSVC C++ toolchain available.

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

Get the triple with `rustc --print host-tuple`. Release assets already carry the
triple in their names. `stage-sidecar.sh` preserves that name and adds the `.exe`
suffix required on Windows, so no manual rename or copy is needed.

Once copied into the build, the triple is stripped again: the sidecar is
`target/{debug,release}/model-proxy-v3`, and inside the bundle
`Proxy-tray.app/Contents/MacOS/model-proxy-v3` (`productName` is `Proxy-tray`).

`cargo check` **requires** the staged sidecar: `tauri_build::build()` copies
`externalBin` during `build.rs` and errors on a missing file.

### Where the staged sidecar comes from

`scripts/stage-sidecar.sh` takes no arguments. It uses `rustc --print host-tuple`
to select an asset from the [latest model_proxy_v3 release](https://github.com/qidu/model_proxy_v3/releases/latest),
downloads it with `curl`, and stages it under the name Tauri expects. HTTP
errors, missing ZIP entries, and empty executables fail the script. Running it
again replaces the staged sidecar with the then-latest release.

On Windows the release is a ZIP. The script extracts the executable,
`keytar.node`, and `keytar-LICENSE.md`; `tauri.windows.conf.json` bundles the
latter two into the install directory beside the sidecar for Credential Vault.
macOS releases contain only the executable.

To check that a staged sidecar answers the tray's control channel:

```sh
printf '{"jsonrpc":"2.0","method":"status.get","params":{},"id":1}\n' \
  | PORT=18788 PROXY_CONFIG_PATH="$PWD/proxy_config.toml" \
    ./src-tauri/binaries/model-proxy-v3-aarch64-apple-darwin --rpc
```

Use the executable name for your host triple. CI performs the same `status.get`
check and additionally requires both Windows companion files.

## CI

`.github/workflows/ci.yml` runs on push to `main`, on pull requests, on
`workflow_dispatch`, and on `release: published` — the last one is the hook for
the web UI's **Create a new release** button, so publishing a release there
builds the installers and attaches them without anything pushed by hand.

Each runner downloads the latest released sidecar for its host triple. The
matrix builds separate Apple Silicon (`macos-latest`, `aarch64-apple-darwin`),
Intel macOS (`macos-15-intel`, `x86_64-apple-darwin`), and Windows x64
(`windows-latest`, `x86_64-pc-windows-msvc`) bundles.

Per platform, the job:

1. Checks out only `proxy_tray`; the proxy submodule is not initialized.
2. Runs `bash scripts/stage-sidecar.sh`, which downloads the latest host-specific
   release. Windows also extracts `keytar.node` and its license from the ZIP.
3. Checks the exact target-triple path and executes the sidecar, requiring a real
   `status.get` response over the same JSON-RPC stdio transport the tray uses.
   The Windows leg also fails if either companion file is missing or empty.
4. Runs `cargo fmt --all --check` (once, on the Apple Silicon leg — formatting is
   a property of the source), `cargo clippy --all-targets -- -D warnings`, and
   `cargo test`.
5. On every event but a pull request, runs `tauri build` and uploads the `.dmg` /
   `.msi` / `-setup.exe` as a workflow artifact named
   `proxy-tray-<target-triple>`; on a published release it also attaches them to
   that release.

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

A normal tray build does not initialize the proxy submodule. These steps are
only for reading its design documentation or developing the proxy source.

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

Normal tray builds do not need any proxy submodule because they download a
published sidecar. For proxy development that needs one of its own submodules
(`submodules/kompress`, `submodules/privacy-filter`, …), clone that repository
**standalone** — a fresh checkout with no stale gitlink to trip over:

```sh
git clone https://github.com/qidu/model_proxy_v3.git   # or the git@ URL with a key
cd model_proxy_v3
git submodule update --init submodules/<name>          # name the path — see below
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

## Windows

Run the whole sequence under [How to build](#how-to-build) on Windows so the
staging script selects the Windows release and Tauri can use the MSVC toolchain.
What follows is Windows-specific.

- **The icon is committed.** `src-tauri\icons\icon.ico` is tracked and listed in
  `bundle.icon`, so a fresh clone builds as-is — `tauri-build` writes the
  executable's Windows resource from it. Run `node scripts\make-icons.mjs` only
  after changing the ring art: it rewrites `icon.png`, the three tray PNGs, and
  the `.ico` (16/32/48/256, the same ring). The macOS build does not use the
  `.ico`.
- **Run `cargo tauri` from the project root** — the directory that holds
  `src-tauri/`, not `src-tauri/` itself: `cargo tauri` locates `src-tauri` by
  searching the cwd and then its children, never its parents.

### Config edits

No manual edits are needed. `tauri.conf.json` contains the shared configuration,
and Tauri automatically merges `tauri.windows.conf.json` for Windows builds.

| Key | Value | Purpose |
| --- | --- | --- |
| `bundle.externalBin` | `["binaries/model-proxy-v3"]` | relative and untagged — Tauri appends `-<triple>`, plus `.exe` on Windows |
| `bundle.icon` | `["icons/icon.png", "icons/icon.ico"]` | Tauri picks whichever the platform's bundler wants |
| Windows `bundle.resources` | `keytar.node` and `keytar-LICENSE.md` mapped to the resource root | places both beside the tray executable and sidecar |

`externalBin` must stay relative and untagged: Tauri resolves it against
`src-tauri` and then appends the triple. An absolute path, or one that already
carries the triple, is wrong (see the naming rule above).

`plugins."proxy-tray".configPath` needs no per-platform value: it is absent by
default and the tray falls back to `~/.config/model-proxy-v3/proxy_config.toml`,
which is the same on every platform.

### Runtime caveats

The staged Windows release includes `keytar.node`, so
`store_key_in_system = true` uses Windows Credential Vault. If that addon is
removed, the proxy warns and falls back to the in-binary body store, which keeps
keys in plaintext inside the executable and needs a writable directory.
`sdk://` routes are also unsupported in the binary — the optional integration
behind them is excluded too (see `WHAT IS NOT IN THE BINARY` in `build-sea.js`).

A start also ensures a Windows Defender inbound allow rule for the bundled
`model-proxy-v3.exe` (`src-tauri/src/firewall.rs`), so a tray-launched proxy is
reachable from other hosts. The rule is program-scoped (any port and protocol)
and named `Model Proxy Inbound`. Adding a rule needs Administrator and the tray
is not elevated, so this is best-effort: the outcome is mirrored to the window's
LOG section and the proxy starts regardless. Without elevation the rule is not
added — run the tray once as Administrator, or add the rule by hand.
