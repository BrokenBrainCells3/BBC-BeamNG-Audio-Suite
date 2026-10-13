//! MCP (Model Context Protocol) client for forwarding inputs to a BeamNG instance.
//!
//! The BeamNG MCP server is an HTTP JSON-RPC 2.0 server (default port 29292)
//! exposing tools like `inject_input`. This module provides a simple async
//! client to call those tools.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

/// Default MCP server port (BeamNG default)
pub(crate) const DEFAULT_MCP_PORT: u16 = 29292;
const MCP_ROUTE: &str = "/mcp";

/// MCP log file lives next to the app config (%APPDATA%\com.audio.app.desktop).
fn log_path() -> PathBuf {
    std::env::var("APPDATA")
        .ok()
        .map(|d| {
            PathBuf::from(d)
                .join("com.audio.app.desktop")
                .join("mcp.log")
        })
        .unwrap_or_else(|| PathBuf::from("mcp.log"))
}

/// Append a timestamped line to the MCP log (and mirror to stderr).
/// Every MCP send attempt — and the reason a send was skipped — goes
/// through here, so "is it reaching the game?" can be answered from file.
pub fn mcp_log(msg: &str) {
    let line = format!(
        "[{}] {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        msg
    );
    eprintln!("{line}");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// JSON-RPC 2.0 request
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: Value,
}

/// JSON-RPC 2.0 response
#[derive(Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Cached MCP client state per Extra App
struct McpClientState {
    client: Client,
    base_url: String,
    request_id: AtomicU64,
    enabled: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl McpClientState {
    fn new(host: String, port: u16) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("MCP client builder failed");
        let base_url = format!("http://{}:{}/mcp", host, port);
        Self {
            client,
            base_url,
            request_id: AtomicU64::new(1),
            enabled: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    async fn call_tool(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: "tools/call".into(),
            params: json!({ "name": tool, "arguments": arguments }),
        };
        mcp_log(&format!(
            "[MCP] POST {} -> {}",
            self.base_url,
            serde_json::to_string(&req).unwrap_or_default()
        ));
        let resp = self
            .client
            .post(&self.base_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("MCP request failed: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| format!("MCP read failed: {e}"))?;
        mcp_log(&format!("[MCP] Response: HTTP {status} -> {body}"));
        if !status.is_success() {
            return Err(format!("MCP HTTP {status}: {body}"));
        }
        let rpc: JsonRpcResponse = serde_json::from_str(&body)
            .map_err(|e| format!("MCP parse failed: {e}"))?;
        if let Some(err) = rpc.error {
            return Err(format!("MCP error {}: {}", err.code, err.message));
        }
        Ok(rpc.result.unwrap_or(Value::Null))
    }

    /// Inject a vehicle input event via MCP
    pub async fn inject_input(
        &self,
        event: &str,
        value: f64,
        filter: Option<i32>,
        vehicle_id: Option<i32>,
    ) -> Result<(), String> {
        let mut args = json!({ "event": event, "value": value });
        if let Some(f) = filter {
            args["filter"] = json!(f);
        }
        if let Some(id) = vehicle_id {
            args["id"] = json!(id);
        }
        mcp_log(&format!(
            "[MCP] Sending inject_input: event={event}, value={value}, filter={filter:?}, vehicle_id={vehicle_id:?}"
        ));
        let result = self.call_tool("inject_input", args).await;
        match &result {
            Ok(_) => mcp_log("[MCP] inject_input succeeded"),
            Err(e) => mcp_log(&format!("[MCP] inject_input failed: {e}")),
        }
        result.map(|_| ())
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_last_error(&self, err: Option<String>) {
        if let Ok(mut e) = self.last_error.lock() {
            *e = err;
        }
    }

    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }
}

/// Global registry of MCP clients per Extra App (by app ID)
static MCP_CLIENTS: OnceLock<Mutex<HashMap<String, Arc<McpClientState>>>> = OnceLock::new();

fn clients_lock() -> &'static Mutex<HashMap<String, Arc<McpClientState>>> {
    MCP_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get or create an MCP client for an app ID
pub fn get_or_create_mcp_client(app_id: &str, host: String, port: u16) -> Arc<McpClientState> {
    let mut clients = clients_lock().lock().unwrap();
    clients
        .entry(app_id.to_string())
        .or_insert_with(|| Arc::new(McpClientState::new(host, port)))
        .clone()
}

/// Remove an MCP client (when app is deleted)
pub fn remove_mcp_client(app_id: &str) {
    clients_lock().lock().unwrap().remove(app_id);
}

/// Enable/disable MCP for an app
pub fn set_mcp_enabled(app_id: &str, enabled: bool) {
    if let Some(client) = clients_lock().lock().unwrap().get(app_id) {
        client.set_enabled(enabled);
    }
}

/// Check if MCP is enabled for an app
pub fn is_mcp_enabled(app_id: &str) -> bool {
    clients_lock()
        .lock()
        .unwrap()
        .get(app_id)
        .map(|c| c.is_enabled())
        .unwrap_or(false)
}

/// Get last error for an app's MCP client
pub fn get_mcp_last_error(app_id: &str) -> Option<String> {
    clients_lock()
        .lock()
        .unwrap()
        .get(app_id)
        .and_then(|c| c.get_last_error())
}

/// Tauri command: configure MCP for an app
#[tauri::command]
pub fn mcp_configure(
    app: AppHandle,
    app_id: String,
    host: Option<String>,
    port: Option<u16>,
    enabled: Option<bool>,
    vehicle_id: Option<i32>,
) -> Result<String, String> {
    let host = host.unwrap_or_else(|| "127.0.0.1".into());
    let port = port.unwrap_or(DEFAULT_MCP_PORT);
    let client = get_or_create_mcp_client(&app_id, host.clone(), port);
    if let Some(e) = enabled {
        client.set_enabled(e);
    }
    // Persist the settings on the embedded app state too: the UDP input
    // relay calls `inject_input_mcp`, which reads mcp_enabled/host/port and
    // the vehicle ID straight from EmbedStateInner. Without this write the
    // forwarding path can never activate ("MCP not enabled for embedded app").
    {
        let state = app.state::<crate::embed::EmbedState>();
        let mut g = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(e) = enabled {
            g.mcp_enabled = e;
        }
        g.mcp_host = host.clone();
        g.mcp_port = port;
        if let Some(v) = vehicle_id {
            g.embedded_vehicle_id = Some(v);
        }
        g.embedded_app_id = Some(app_id.clone());
    }
    mcp_log(&format!(
        "[MCP] mcp_configure app_id={app_id} host={host} port={port} enabled={:?} vehicle_id={vehicle_id:?}",
        enabled
    ));
    Ok(format!("MCP configured for {app_id}"))
}

/// Tauri command: send an input event via MCP
#[tauri::command]
pub async fn mcp_inject_input(
    app_id: String,
    event: String,
    value: f64,
    filter: Option<i32>,
    vehicle_id: Option<i32>,
) -> Result<String, String> {
    let client = clients_lock()
        .lock()
        .unwrap()
        .get(&app_id)
        .cloned()
        .ok_or_else(|| format!("No MCP client for app {app_id}"))?;
    client
        .inject_input(&event, value, filter, vehicle_id)
        .await
        .map(|_| "sent".into())
}

/// Tauri command: get MCP status for an app
#[tauri::command]
pub fn mcp_status(app_id: String) -> serde_json::Value {
    let client = clients_lock().lock().unwrap().get(&app_id).cloned();
    match client {
        Some(c) => json!({
            "enabled": c.is_enabled(),
            "last_error": c.get_last_error(),
        }),
        None => json!({ "enabled": false, "last_error": null }),
    }
}

/// Get the embedded app's MCP settings from the embed state
pub fn get_embedded_mcp_settings(app: &tauri::AppHandle) -> Option<(bool, String, u16)> {
    app.state::<crate::embed::EmbedState>()
        .0
        .lock()
        .ok()
        .and_then(|g| {
            if g.mcp_enabled {
                Some((g.mcp_enabled, g.mcp_host.clone(), g.mcp_port))
            } else {
                None
            }
        })
}

/// Get the embedded app's vehicle ID from the embed state
pub fn get_embedded_vehicle_id(app: &tauri::AppHandle) -> Option<i32> {
    app.state::<crate::embed::EmbedState>()
        .0
        .lock()
        .ok()
        .and_then(|g| g.embedded_vehicle_id)
}

/// Get the embedded app ID from the embed state
pub fn get_embedded_app_id(app: &tauri::AppHandle) -> Option<String> {
    app.state::<crate::embed::EmbedState>()
        .0
        .lock()
        .ok()
        .and_then(|g| g.embedded_app_id.clone())
}

/// Inject an input event via MCP using the embedded app's MCP settings
pub async fn inject_input_mcp(app: &tauri::AppHandle, event: &str, value: f64, filter: Option<i32>, vehicle_id: Option<i32>) -> Result<(), String> {
    let settings = get_embedded_mcp_settings(app);
    let (enabled, host, port) = match settings {
        Some(s) => s,
        None => {
            mcp_log(&format!(
                "[MCP] inject_input_mcp '{event}' SKIPPED: no MCP settings on embedded state (mcp_enabled=false or state locked) — was mcp_configure ever called?"
            ));
            return Err("MCP not enabled for embedded app".to_string());
        }
    };

    if !enabled {
        mcp_log(&format!(
            "[MCP] inject_input_mcp '{event}' SKIPPED: mcp_enabled=false (host={host} port={port})"
        ));
        return Err("MCP not enabled".into());
    }

    // Use the stored vehicle ID if not explicitly provided
    let vehicle_id = vehicle_id.or_else(|| get_embedded_vehicle_id(app));

    let client = get_or_create_mcp_client("embedded", host.clone(), port);
    client.set_enabled(true);
    mcp_log(&format!(
        "[MCP] inject_input_mcp '{event}' -> http://{host}:{port}{MCP_ROUTE} (value={value}, filter={filter:?}, vehicle_id={vehicle_id:?})"
    ));
    let result = client.inject_input(event, value, filter, vehicle_id).await;
    match &result {
        Ok(_) => mcp_log(&format!("[MCP] inject_input_mcp '{event}' delivered")),
        Err(e) => mcp_log(&format!("[MCP] inject_input_mcp '{event}' failed: {e}")),
    }
    result
}

/// Tauri command: test MCP connection
#[tauri::command]
pub async fn mcp_test_connection(host: String, port: u16) -> Result<String, String> {
    mcp_log(&format!("[MCP] Testing connection to {host}:{port}"));
    let client = McpClientState::new(host, port);
    client.set_enabled(true);
    match client.inject_input("test", 0.0, Some(0), None).await {
        Ok(_) => Ok("MCP connection successful".into()),
        Err(e) => Err(format!("MCP connection failed: {}", e)),
    }
}