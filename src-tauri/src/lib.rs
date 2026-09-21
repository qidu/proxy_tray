//! Tray app that supervises the `model-proxy-v3` SEA binary.
//!
//! The app owns the proxy process and drives it over the JSON-RPC control
//! channel on stdio (`--rpc`, doc §3). It never proxies model traffic itself:
//! clients keep talking HTTP to the proxy's own port. This file is the
//! supervisor, the tray and the window's command surface; `rpc.rs` is the
//! protocol half.

mod rpc;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuEvent, MenuItem, MenuItemBuilder, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

use rpc::RpcClient;

/// Tray art. `image-png` is a non-default tauri feature — without it these
/// compile but fail to decode at runtime; see src-tauri/Cargo.toml.
const ICON_RUNNING: &[u8] = include_bytes!("../icons/tray-running.png");
const ICON_STOPPED: &[u8] = include_bytes!("../icons/tray-stopped.png");
const ICON_ERROR: &[u8] = include_bytes!("../icons/tray-error.png");

/// The proxy's own default port (`src/server.ts`).
const DEFAULT_PORT: u16 = 8788;
/// How long to wait for the port to be released after a kill before spawning
/// anyway, and how often to re-check.
const PORT_FREE_TIMEOUT: Duration = Duration::from_secs(5);
const PORT_FREE_POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq)]
enum ProxyStatus {
    Stopped,
    Running,
    /// The child could not be spawned, or exited on its own with an error.
    Failed(String),
}

impl ProxyStatus {
    fn is_running(&self) -> bool {
        matches!(self, ProxyStatus::Running)
    }
}

struct ProxyState {
    rpc: Arc<RpcClient>,
    /// Absolute path handed to the child, and shown in the window (doc §7).
    config_path: Option<PathBuf>,
    port: u16,
    status: Mutex<ProxyStatus>,
    /// `version` from the last successful `status.get`; `None` until then.
    proxy_version: Mutex<Option<String>>,
    /// Message from the last failed `config.reload`. Drives the amber icon
    /// even while the proxy keeps running (doc §5).
    reload_error: Mutex<Option<String>>,
}

impl ProxyState {
    fn status(&self) -> ProxyStatus {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, status: ProxyStatus) {
        *self.status.lock().unwrap() = status;
    }
}

/// Handles to the two menu items whose text tracks the status. The menu itself
/// is not kept — the `TrayIcon` owns its own copy.
struct TrayMenu {
    status: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
}

/// Clone the shared state out of an app handle.
///
/// `State<'_, T>` borrows the handle, so it cannot cross an `.await` in an
/// async command or a spawned task. Cloning the `Arc` gives an owned handle.
fn state_of(app: &AppHandle) -> Arc<ProxyState> {
    let state = app.state::<Arc<ProxyState>>();
    Arc::clone(state.inner())
}

/// Resolve the config the proxy will read (doc §7, "Pass explicitly").
///
/// `PROXY_CONFIG_PATH` wins; otherwise the absolute path in `tauri.conf.json`
/// under `plugins."proxy-tray".configPath`. Never derived from the working
/// directory: a Finder launch has cwd `/`, so a relative default resolves to a
/// path that does not exist and the proxy silently reads the wrong file.
///
/// `None` means neither source named a config. The proxy's own default is
/// `./proxy_config.toml` (`server.ts:45`), which is cwd-relative too, so the
/// caller must not paper over this with an empty `PROXY_CONFIG_PATH` — an empty
/// string is falsy and would be discarded by that same `||` fallback.
fn resolve_config_path(config: &tauri::Config) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PROXY_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }
    let configured = config
        .plugins
        .0
        .get("proxy-tray")
        .and_then(|plugin| plugin.get("configPath"))
        .and_then(serde_json::Value::as_str);
    match configured {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!(
                "[tray] no PROXY_CONFIG_PATH and no plugins.\"proxy-tray\".configPath in \
                 tauri.conf.json: the proxy will use its own working-directory default"
            );
            None
        }
    }
}

/// Resolve the port the proxy listens on.
///
/// `PORT` wins, then `plugins."proxy-tray".port` in `tauri.conf.json`, then
/// [`DEFAULT_PORT`]. The proxy itself has no port setting — it reads `PORT` from
/// the environment (`server.ts:13`) — so this is the only knob, and the file
/// fallback is what lets a Finder launch choose a port at all.
fn resolve_port(config: &tauri::Config) -> u16 {
    if let Ok(raw) = std::env::var("PORT") {
        match raw.parse() {
            Ok(port) => return port,
            Err(err) => eprintln!("[tray] ignoring PORT={raw:?} ({err})"),
        }
    }

    match config
        .plugins
        .0
        .get("proxy-tray")
        .and_then(|plugin| plugin.get("port"))
        .and_then(Value::as_u64)
    {
        Some(port) if port <= u16::MAX as u64 => port as u16,
        Some(port) => {
            eprintln!(
                "[tray] ignoring plugins.\"proxy-tray\".port={port}: not a valid port; \
                 using {DEFAULT_PORT}"
            );
            DEFAULT_PORT
        }
        None => DEFAULT_PORT,
    }
}

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch hands off to the running app instead of
        // starting a second proxy that would fight over the same port (doc §7).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            proxy_status,
            proxy_start,
            proxy_stop,
            proxy_restart,
            rpc_call,
            reload_config,
            export_provider,
            open_dashboard,
        ])
        .setup(|app| {
            app.manage(Arc::new(ProxyState {
                rpc: RpcClient::new(),
                config_path: resolve_config_path(app.config()),
                port: resolve_port(app.config()),
                status: Mutex::new(ProxyStatus::Stopped),
                proxy_version: Mutex::new(None),
                reload_error: Mutex::new(None),
            }));
            app.manage(build_tray(app)?);
            hide_on_close(app.handle());
            // A failed autostart is not fatal: the tray goes amber and the
            // window shows why, which beats an app that will not launch.
            if let Err(err) = spawn_proxy(app.handle().clone()) {
                eprintln!("[tray] {err}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Build the tray icon and its menu.
///
/// Built here rather than through the `app.trayIcon` config key, which would
/// create a second, menu-less icon alongside this one.
fn build_tray(app: &tauri::App) -> tauri::Result<TrayMenu> {
    let status = MenuItemBuilder::with_id("status", "Stopped")
        .enabled(false)
        .build(app)?;
    let toggle = MenuItemBuilder::with_id("toggle", "Start").build(app)?;
    let restart = MenuItemBuilder::with_id("restart", "Restart").build(app)?;
    let open = MenuItemBuilder::with_id("open_dashboard", "Open Dashboard").build(app)?;
    let reload = MenuItemBuilder::with_id("reload_config", "Reload Config").build(app)?;

    let export_pi = MenuItemBuilder::with_id("export_pi", "for pi").build(app)?;
    let export_openclaw = MenuItemBuilder::with_id("export_openclaw", "for openclaw").build(app)?;
    let export = SubmenuBuilder::with_id(app, "export", "Export Provider Config")
        .item(&export_pi)
        .item(&export_openclaw)
        .build()?;

    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&status)
        .item(&toggle)
        .item(&restart)
        .separator()
        .item(&open)
        .item(&reload)
        .item(&export)
        .separator()
        .item(&quit)
        .build()?;

    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON_STOPPED)?)
        .icon_as_template(false)
        .tooltip("model_proxy_v3")
        .menu(&menu)
        // Left click opens the window; the menu is bound to right click (§5).
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event)
        .build(app)?;

    Ok(TrayMenu { status, toggle })
}

/// Re-derive the tray icon, tooltip and status label from the current state,
/// then tell the window. Every state change funnels through here so the tray
/// and the process state cannot disagree.
fn refresh_tray(app: &AppHandle, state: &Arc<ProxyState>) {
    let status = state.status();
    let reload_error = state.reload_error.lock().unwrap().clone();
    let version = state.proxy_version.lock().unwrap().clone();

    let (icon, label, error) = match &status {
        ProxyStatus::Running => {
            let label = match &version {
                Some(version) => format!("Running on :{} · v{}", state.port, version),
                None => format!("Running on :{}", state.port),
            };
            (ICON_RUNNING, label, None)
        }
        ProxyStatus::Stopped => (ICON_STOPPED, "Stopped".to_string(), None),
        ProxyStatus::Failed(err) => (ICON_ERROR, format!("Error: {err}"), Some(err.clone())),
    };

    // A failed reload is amber even while the proxy runs (doc §5).
    let icon = if reload_error.is_some() {
        ICON_ERROR
    } else {
        icon
    };
    let label = match (&reload_error, status.is_running()) {
        (Some(err), true) => format!("{label} — reload failed: {err}"),
        _ => label,
    };

    match app.tray_by_id("main") {
        Some(tray) => {
            match Image::from_bytes(icon) {
                Ok(image) => {
                    let _ = tray.set_icon_with_as_template(Some(image), false);
                }
                Err(err) => eprintln!("[tray] could not decode the tray icon: {err}"),
            }
            let _ = tray.set_tooltip(Some(label.as_str()));
        }
        None => eprintln!("[tray] no tray icon registered under id \"main\""),
    }

    match app.try_state::<TrayMenu>() {
        Some(menu) => {
            let _ = menu.status.set_text(&label);
            let _ = menu
                .toggle
                .set_text(if status.is_running() { "Stop" } else { "Start" });
        }
        None => eprintln!("[tray] no tray menu in state"),
    }

    let _ = app.emit(
        "proxy://status",
        json!({
            "running": status.is_running(),
            "port": state.port,
            "label": label,
            "error": error,
            "reloadError": reload_error,
        }),
    );
}

/// Spawn the sidecar and pump its stdout into the RPC client.
fn spawn_proxy(app: AppHandle) -> Result<(), String> {
    let state = state_of(&app);
    if state.rpc.is_attached() {
        return Err("the proxy is already running".to_string());
    }

    let command = match app.shell().sidecar("model-proxy-v3") {
        Ok(command) => {
            let command = command.args(["--rpc"]).env("PORT", state.port.to_string());
            // Omit the variable entirely when unset: an empty `PROXY_CONFIG_PATH`
            // is falsy and `server.ts:45`'s `||` would silently swap in
            // `./proxy_config.toml`.
            match &state.config_path {
                Some(path) => command.env("PROXY_CONFIG_PATH", path.to_string_lossy().to_string()),
                None => command,
            }
        }
        Err(err) => {
            return Err(fail_start(
                &app,
                &state,
                format!("could not locate the sidecar: {err}"),
            ))
        }
    };

    let (mut events, child) = match command.spawn() {
        Ok(spawned) => spawned,
        Err(err) => {
            return Err(fail_start(
                &app,
                &state,
                format!("could not start the proxy: {err}"),
            ))
        }
    };

    state.rpc.attach(child);
    state.set_status(ProxyStatus::Running);
    refresh_tray(&app, &state);

    let reader_state = Arc::clone(&state);
    let reader_app = app.clone();
    let rpc = Arc::clone(&state.rpc);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(bytes) => {
                    let line = String::from_utf8_lossy(&bytes);
                    if let Some(notification) = rpc.handle_line(&line) {
                        let _ = reader_app.emit("proxy://notification", notification);
                    }
                }
                // stdout is the frame channel, so logs must stay on stderr (§3).
                CommandEvent::Stderr(bytes) => {
                    eprintln!("[proxy] {}", String::from_utf8_lossy(&bytes));
                }
                CommandEvent::Error(err) => eprintln!("[proxy] stream error: {err}"),
                CommandEvent::Terminated(payload) => {
                    eprintln!(
                        "[proxy] exited (code {:?}, signal {:?})",
                        payload.code, payload.signal
                    );
                    rpc.take_child();
                    rpc.fail_pending("the proxy exited");
                    *reader_state.proxy_version.lock().unwrap() = None;
                    reader_state.set_status(ProxyStatus::Stopped);
                    refresh_tray(&reader_app, &reader_state);
                    break;
                }
                _ => {}
            }
        }
    });

    // `status.get` only answers once the HTTP socket is bound, so this both
    // fills in the version and confirms the port is live (doc §3). It must run
    // in its own task: the reader above is the only thing pumping stdout, so
    // awaiting a reply from inside it would deadlock.
    let probe_state = Arc::clone(&state);
    let probe_rpc = Arc::clone(&state.rpc);
    tauri::async_runtime::spawn(async move {
        match probe_rpc.call("status.get", json!({})).await {
            Ok(result) => {
                if let Some(version) = result.get("version").and_then(Value::as_str) {
                    *probe_state.proxy_version.lock().unwrap() = Some(version.to_string());
                }
                refresh_tray(&app, &probe_state);
            }
            Err(err) => eprintln!("[tray] status.get failed: {err}"),
        }
    });

    Ok(())
}

/// Record a failed start and refresh the tray, handing the message back so the
/// caller surfaces it: the window shows it, the menu logs it.
fn fail_start(app: &AppHandle, state: &Arc<ProxyState>, message: String) -> String {
    state.set_status(ProxyStatus::Failed(message.clone()));
    refresh_tray(app, state);
    message
}

/// Stop the proxy: ask it to shut down, then make sure it is gone.
async fn stop_proxy(app: &AppHandle, state: &Arc<ProxyState>) {
    if !state.rpc.is_attached() {
        state.set_status(ProxyStatus::Stopped);
        refresh_tray(app, state);
        return;
    }

    // `shutdown` replies `{ok}` and *then* exits (src/rpc.ts), so a successful
    // reply is not proof it is gone — the kill is the backstop.
    if let Err(err) = Arc::clone(&state.rpc).call("shutdown", json!({})).await {
        eprintln!("[tray] shutdown call failed ({err}); killing the child");
    }

    if state.rpc.kill() {
        state.rpc.fail_pending("the proxy was stopped");
    }
    *state.proxy_version.lock().unwrap() = None;
    state.set_status(ProxyStatus::Stopped);
    refresh_tray(app, state);
}

async fn restart_proxy(app: &AppHandle, state: &Arc<ProxyState>) -> Result<(), String> {
    stop_proxy(app, state).await;
    // The child exits asynchronously, so the port may still be held. Spawning
    // into a held port fails `listen`, so RPC never starts and the child dies
    // (doc §3) — waiting is the difference between a restart and a crash.
    wait_for_port_free(state.port).await;
    spawn_proxy(app.clone())
}

/// Wait, bounded, for the proxy's port to be released after a kill.
///
/// The timeout keeps a socket stuck in TIME_WAIT from hanging the tray forever;
/// if it expires the spawn is attempted anyway and its failure is reported.
async fn wait_for_port_free(port: u16) {
    let deadline = tokio::time::Instant::now() + PORT_FREE_TIMEOUT;
    loop {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            eprintln!(
                "[tray] port {port} is still busy after {PORT_FREE_TIMEOUT:?}; starting anyway"
            );
            return;
        }
        tokio::time::sleep(PORT_FREE_POLL).await;
    }
}

/// Reload the config from the local file and record whether it worked, so the
/// tray icon can go amber (doc §5).
async fn reload_config_impl(app: &AppHandle, state: &Arc<ProxyState>) {
    let outcome = if state.rpc.is_attached() {
        Arc::clone(&state.rpc)
            .call("config.reload", json!({}))
            .await
            .map(|_| ())
    } else {
        Err(rpc::RpcError::new(
            -32603,
            "the proxy is not running, so there is nothing to reload",
        ))
    };

    let message = outcome.err().map(|err| {
        eprintln!("[tray] config.reload failed: {err}");
        err.to_string()
    });
    *state.reload_error.lock().unwrap() = message;
    refresh_tray(app, state);
}

/// Run one of the CLI helpers and hand its stdout to the window.
///
/// The Rust `Command` has no `output()`, so the output is collected from the
/// event stream until `Terminated` (doc §4).
async fn run_export(app: &AppHandle, kind: &str) {
    let flag = match kind {
        "pi" => "--export-pi-models",
        "openclaw" => "--export-openclaw-providers",
        other => {
            let message = format!("unknown export kind {other:?}");
            eprintln!("[tray] {message}");
            emit_export(app, kind, "", Some(message));
            return;
        }
    };

    let command = match app.shell().sidecar("model-proxy-v3") {
        Ok(command) => {
            let command = command.args([flag]);
            // The export reads the same config as the proxy, so it needs the
            // same `PROXY_CONFIG_PATH`: without it the CLI falls back to the
            // cwd-relative `./proxy_config.toml` and fails under a Finder
            // launch (doc §7). Omit the variable when unset, as in `spawn_proxy`.
            match &state_of(app).config_path {
                Some(path) => command.env("PROXY_CONFIG_PATH", path.to_string_lossy().to_string()),
                None => command,
            }
        }
        Err(err) => {
            let message = format!("could not locate the sidecar: {err}");
            eprintln!("[tray] {message}");
            emit_export(app, kind, "", Some(message));
            return;
        }
    };

    let (mut events, child) = match command.spawn() {
        Ok(spawned) => spawned,
        Err(err) => {
            let message = format!("could not run the export: {err}");
            eprintln!("[tray] {message}");
            emit_export(app, kind, "", Some(message));
            return;
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut code = None;
    let mut terminated = false;
    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Stdout(bytes) => {
                stdout.push_str(&String::from_utf8_lossy(&bytes));
                stdout.push('\n');
            }
            CommandEvent::Stderr(bytes) => {
                stderr.push_str(&String::from_utf8_lossy(&bytes));
                stderr.push('\n');
            }
            CommandEvent::Error(err) => stderr.push_str(&format!("{err}\n")),
            CommandEvent::Terminated(payload) => {
                code = payload.code;
                terminated = true;
                break;
            }
            _ => {}
        }
    }

    // Fail loud: anything but a clean exit means the printed block is not
    // trustworthy, so the window gets the error instead of a silent success.
    let error = if !terminated {
        let _ = child.kill();
        Some(format!("the export did not terminate: {}", stderr.trim()))
    } else if code != Some(0) {
        Some(format!(
            "the export exited with code {code:?}: {}",
            stderr.trim()
        ))
    } else {
        None
    };
    if let Some(err) = &error {
        eprintln!("[tray] {err}");
    }
    emit_export(app, kind, stdout.trim_end(), error);
}

fn emit_export(app: &AppHandle, kind: &str, output: &str, error: Option<String>) {
    let _ = app.emit(
        "proxy://export",
        json!({ "kind": kind, "output": output, "error": error }),
    );
}

fn open_dashboard_impl(app: &AppHandle, state: &Arc<ProxyState>) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/dashboard", state.port);
    app.opener()
        .open_url(url.clone(), None::<&str>)
        .map_err(|err| format!("could not open {url}: {err}"))
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    let app = app.clone();
    let state = state_of(&app);

    tauri::async_runtime::spawn(async move {
        match id.as_str() {
            "toggle" => {
                if state.status().is_running() {
                    stop_proxy(&app, &state).await;
                } else if let Err(err) = spawn_proxy(app.clone()) {
                    eprintln!("[tray] {err}");
                }
            }
            "restart" => {
                if let Err(err) = restart_proxy(&app, &state).await {
                    eprintln!("[tray] {err}");
                }
            }
            "reload_config" => reload_config_impl(&app, &state).await,
            "open_dashboard" => {
                if let Err(err) = open_dashboard_impl(&app, &state) {
                    eprintln!("[tray] {err}");
                }
            }
            "export_pi" => run_export(&app, "pi").await,
            "export_openclaw" => run_export(&app, "openclaw").await,
            "quit" => {
                stop_proxy(&app, &state).await;
                app.exit(0);
            }
            other => eprintln!("[tray] unhandled menu id {other:?}"),
        }
    });
}

/// Left click opens the window; the menu is on right click (doc §5).
fn on_tray_icon_event(tray: &TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_window(tray.app_handle());
    }
}

fn show_window(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        None => eprintln!("[tray] no window labelled \"main\""),
    }
}

/// Closing the window hides it; only Quit stops the proxy (doc §5).
fn hide_on_close(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        eprintln!("[tray] no window labelled \"main\" to hook");
        return;
    };
    let window_to_hide = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_to_hide.hide();
        }
    });
}

/// Current status for the window.
///
/// Falls back to a stopped-shaped answer when the child is down, so the UI never
/// has to special-case an RPC failure. The resolved config path and any reload
/// error ride along because the window shows both.
#[tauri::command]
async fn proxy_status(app: AppHandle) -> Result<Value, String> {
    let state = state_of(&app);

    let mut payload = if state.rpc.is_attached() {
        match Arc::clone(&state.rpc).call("status.get", json!({})).await {
            Ok(result) => result,
            Err(err) => json!({ "running": false, "error": err.to_string() }),
        }
    } else {
        json!({ "running": false })
    };

    let Some(object) = payload.as_object_mut() else {
        return Err(format!("status.get returned a non-object: {payload}"));
    };
    object.insert("port".into(), json!(state.port));
    object.insert(
        "configPath".into(),
        json!(state
            .config_path
            .as_ref()
            .map(|path| path.to_string_lossy())),
    );
    object.insert(
        "reloadError".into(),
        json!(state.reload_error.lock().unwrap().clone()),
    );
    Ok(payload)
}

#[tauri::command]
fn proxy_start(app: AppHandle) -> Result<(), String> {
    spawn_proxy(app)
}

#[tauri::command]
async fn proxy_stop(app: AppHandle) -> Result<(), String> {
    let state = state_of(&app);
    stop_proxy(&app, &state).await;
    Ok(())
}

#[tauri::command]
async fn proxy_restart(app: AppHandle) -> Result<(), String> {
    let state = state_of(&app);
    restart_proxy(&app, &state).await
}

/// Thin pass-through so the window can reach the rest of the doc §3 surface
/// (`models.list`, `config.get`, `stats.*`, `quota.get`, …) without a command
/// per method.
#[tauri::command]
async fn rpc_call(app: AppHandle, method: String, params: Option<Value>) -> Result<Value, String> {
    let state = state_of(&app);
    Arc::clone(&state.rpc)
        .call(method, params.unwrap_or_else(|| json!({})))
        .await
        .map_err(|err| err.to_string())
}

/// Same action as the tray's "Reload Config", so the window's button drives the
/// amber icon too instead of being a second, divergent implementation.
#[tauri::command]
async fn reload_config(app: AppHandle) -> Result<(), String> {
    let state = state_of(&app);
    reload_config_impl(&app, &state).await;
    Ok(())
}

#[tauri::command]
async fn export_provider(app: AppHandle, kind: String) -> Result<(), String> {
    run_export(&app, &kind).await;
    Ok(())
}

#[tauri::command]
fn open_dashboard(app: AppHandle) -> Result<(), String> {
    let state = state_of(&app);
    open_dashboard_impl(&app, &state)
}
