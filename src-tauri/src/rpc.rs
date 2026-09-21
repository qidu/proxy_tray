//! JSON-RPC 2.0 client for the proxy's `--rpc` control channel.
//!
//! The Rust half of `model_proxy_v3/src/rpc.ts`: one JSON object per line out on
//! the child's stdin, responses and notifications back on its stdout. Requests
//! are correlated by a monotonically increasing integer `id`; a frame carrying
//! `method` and no `id` is a notification. See docs/design_tauri_tray.md §3.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri_plugin_shell::process::CommandChild;
use tokio::sync::oneshot;

/// JSON-RPC's reserved "internal error" code. Also used for transport failures,
/// which have no server-side code of their own.
const INTERNAL_ERROR: i64 = -32603;

/// A JSON-RPC error: either one the proxy reported, or a transport failure.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>;

/// The client side of the control channel, shared by the tray, the window
/// commands and the stdout reader.
#[derive(Default)]
pub struct RpcClient {
    child: Mutex<Option<CommandChild>>,
    next_id: AtomicU64,
    pending: Mutex<Pending>,
}

impl RpcClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Take ownership of a freshly spawned child.
    pub fn attach(&self, child: CommandChild) {
        *self.child.lock().unwrap() = Some(child);
    }

    pub fn is_attached(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    /// Detach the child without killing it — used once it has already exited.
    pub fn take_child(&self) -> Option<CommandChild> {
        self.child.lock().unwrap().take()
    }

    /// Kill the child. Returns false when there was none to kill.
    pub fn kill(&self) -> bool {
        match self.take_child() {
            Some(child) => {
                if let Err(err) = child.kill() {
                    eprintln!("[tray] could not kill the proxy: {err}");
                }
                true
            }
            None => false,
        }
    }

    /// Send one request and await its reply.
    ///
    /// Takes `self: Arc<Self>` so callers get an owned, `'static` future — what
    /// `#[tauri::command] async fn` and `async_runtime::spawn` require.
    pub async fn call(
        self: Arc<Self>,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Value, RpcError> {
        let method = method.into();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);

        let frame = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": id });
        let written = {
            // The guard lives only inside this block: no `MutexGuard` may be held
            // across the `.await` below, or the reply could never resolve it.
            let mut child = self.child.lock().unwrap();
            match child.as_mut() {
                Some(child) => child.write(format!("{frame}\n").as_bytes()).map_err(|err| {
                    RpcError::new(INTERNAL_ERROR, format!("write to the proxy failed: {err}"))
                }),
                None => Err(RpcError::new(INTERNAL_ERROR, "the proxy is not running")),
            }
        };
        if let Err(err) = written {
            self.pending.lock().unwrap().remove(&id);
            return Err(err);
        }

        match receiver.await {
            Ok(reply) => reply,
            // The sender is dropped without sending only when the child is gone
            // (`fail_pending`, or a detached child), so this is an exit.
            Err(_) => Err(RpcError::new(
                INTERNAL_ERROR,
                "the proxy exited before replying",
            )),
        }
    }

    /// Feed one stdout line. A reply resolves its waiting `call`; a notification
    /// (a `method` with no `id`) is handed back to the caller to forward.
    ///
    /// Nothing is dropped silently: a frame matching no in-flight request is
    /// reported on stderr rather than discarded.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let message: Value = match serde_json::from_str(trimmed) {
            Ok(message) => message,
            Err(err) => {
                eprintln!("[proxy] unparseable stdout line ({err}): {trimmed}");
                return None;
            }
        };

        if message.get("method").is_some() && message.get("id").is_none() {
            return Some(message);
        }

        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            // A parse error or invalid request is answered with `"id": null`
            // (doc §3), so there is no pending call to resolve — surface it.
            eprintln!("[proxy] unsolicited frame: {message}");
            return None;
        };

        let Some(sender) = self.pending.lock().unwrap().remove(&id) else {
            eprintln!("[proxy] reply for unknown request id {id}: {message}");
            return None;
        };

        let outcome = match message.get("error") {
            Some(error) => Err(RpcError::new(
                error
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or(INTERNAL_ERROR),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the proxy returned an error")
                    .to_string(),
            )),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = sender.send(outcome);
        None
    }

    /// Fail every in-flight request: the child is gone, so no reply can arrive.
    pub fn fail_pending(&self, reason: &str) {
        let in_flight: Vec<_> = self.pending.lock().unwrap().drain().collect();
        for (_, sender) in in_flight {
            let _ = sender.send(Err(RpcError::new(INTERNAL_ERROR, reason)));
        }
    }
}
