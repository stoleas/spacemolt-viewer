// SpaceMolt Viewer — Tauri commands (frontend → backend IPC).
//
// Defines `#[tauri::command]` functions that the frontend calls to manage
// accounts and connections. The frontend invokes these via `invoke()` from
// `@tauri-apps/api/core`.
//
// State model (Decision #5):
//   - `account_manager: Mutex<AccountManager>` — persistent account list +
//     Stronghold credential store
//   - `sessions: Mutex<HashMap<String, AccountSession>>` — live connections,
//     keyed by username
//
// Connection flow (Decision #10 — serial queue with 2s inter-account delay):
//   1. Frontend calls `connect_account(username)` — no password from frontend
//   2. Password is retrieved from Stronghold via `AccountManager::get_password()`
//   3. `AccountSession::connect()` is called BEFORE inserting into the HashMap
//      (lock held only for insert, not during network ops)
//   4. GSM is spawned with the WS receivers + MCP session
//   5. `AccountSession` is inserted into the HashMap
//
// The serial connection queue ensures only one account connects at a time,
// with a 2s delay between accounts, to avoid 4003 rate-limited close codes.

use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;
use tracing::info;

use crate::account_manager::{AccountInfo, AccountManager};
use crate::account_session::AccountSession;
use crate::credential_store::CredentialStore;
use crate::game_state::GameStateManager;

/// App state shared across all Tauri commands.
pub struct AppState {
    /// Account list + Stronghold credential store.
    pub account_manager: Mutex<AccountManager>,
    /// Live connections keyed by username.
    pub sessions: Mutex<HashMap<String, AccountSession>>,
    /// Serial connection queue — ensures only one account connects at a time
    /// with a 2s inter-account delay (Decision #10).
    pub connect_queue: Mutex<()>,
}

/// Inter-account delay to avoid 4003 rate-limited close codes (Decision #10).
const INTER_ACCOUNT_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

// === Credential store commands ===

/// Check if the Stronghold credential store has been initialized.
#[tauri::command]
async fn credentials_initialized(state: State<'_, AppState>) -> Result<bool, String> {
    let mgr = state.account_manager.lock().await;
    Ok(mgr.credentials_initialized())
}

/// Initialize the Stronghold credential store with a master password.
/// This must be called before any account operations.
#[tauri::command]
async fn init_credentials(
    state: State<'_, AppState>,
    password: String,
) -> Result<(), String> {
    let mut mgr = state.account_manager.lock().await;
    mgr.init_credentials(&password).map_err(|e| e.to_string())?;
    mgr.save().await.map_err(|e| e.to_string())?;
    mgr.save_credentials().map_err(|e| e.to_string())?;
    info!("Credential store initialized");
    Ok(())
}

/// Load the account list from disk (accounts.json) and initialize the
/// credential store. Called on app launch if a stronghold snapshot exists.
#[tauri::command]
async fn load_accounts(state: State<'_, AppState>, password: String) -> Result<(), String> {
    let mut mgr = state.account_manager.lock().await;
    if !mgr.credentials_initialized() {
        mgr.init_credentials(&password).map_err(|e| e.to_string())?;
    }
    mgr.load().await.map_err(|e| e.to_string())?;
    info!(count = mgr.account_count(), "Accounts loaded");
    Ok(())
}

// === Account CRUD commands ===

/// List all configured accounts.
#[tauri::command]
async fn get_accounts(state: State<'_, AppState>) -> Result<Vec<AccountInfo>, String> {
    let mgr = state.account_manager.lock().await;
    Ok(mgr.list_accounts().to_vec())
}

/// Add a new account. Password is stored in Stronghold (encrypted at rest).
#[tauri::command]
async fn add_account(
    state: State<'_, AppState>,
    display_name: String,
    username: String,
    password: String,
) -> Result<(), String> {
    let mut mgr = state.account_manager.lock().await;
    mgr.add_account(&display_name, &username, &password)
        .map_err(|e| e.to_string())?;
    mgr.save().await.map_err(|e| e.to_string())?;
    mgr.save_credentials().map_err(|e| e.to_string())?;
    info!(username, display_name, "Account added via command");
    Ok(())
}

/// Remove an account and disconnect if active.
#[tauri::command]
async fn remove_account(
    _app: AppHandle,
    state: State<'_, AppState>,
    username: String,
) -> Result<(), String> {
    // Disconnect if active
    {
        let mut sessions = state.sessions.lock().await;
        if let Some(mut session) = sessions.remove(&username) {
            session.disconnect().await;
            info!(username, "Disconnected during account removal");
        }
    }

    let mut mgr = state.account_manager.lock().await;
    mgr.remove_account(&username).map_err(|e| e.to_string())?;
    mgr.save().await.map_err(|e| e.to_string())?;
    mgr.save_credentials().map_err(|e| e.to_string())?;
    info!(username, "Account removed");
    Ok(())
}

/// Update the password for an existing account.
#[tauri::command]
async fn update_password(
    state: State<'_, AppState>,
    username: String,
    new_password: String,
) -> Result<(), String> {
    let mut mgr = state.account_manager.lock().await;
    mgr.update_password(&username, &new_password)
        .map_err(|e| e.to_string())?;
    mgr.save_credentials().map_err(|e| e.to_string())?;
    info!(username, "Password updated");
    Ok(())
}

// === Connection commands ===

/// Connect an account. Password is retrieved from Stronghold — the frontend
/// never sends passwords for connection (only for initial add).
///
/// Flow:
///   1. Retrieve password from Stronghold
///   2. Acquire serial connection queue lock (Decision #10)
///   3. `AccountSession::connect()` (WS + MCP login)
///   4. Spawn GameStateManager with WS receivers + MCP session
///   5. Insert AccountSession into HashMap
///   6. Wait 2s before releasing the queue lock (inter-account delay)
#[tauri::command]
async fn connect_account(
    app: AppHandle,
    state: State<'_, AppState>,
    username: String,
) -> Result<(), String> {
    // Check if already connected
    {
        let sessions = state.sessions.lock().await;
        if sessions.contains_key(&username) {
            return Err(format!("Account '{username}' is already connected"));
        }
    }

    // Retrieve password from Stronghold
    let password = {
        let mgr = state.account_manager.lock().await;
        mgr.get_password(&username)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No password stored for '{username}'"))?
    };

    // Acquire serial connection queue lock (Decision #10)
    let _queue_guard = state.connect_queue.lock().await;

    info!(username, "Starting connection (serial queue acquired)");

    // Connect (WS + MCP login) — before inserting into HashMap
    let mut session = AccountSession::new();
    session
        .connect(&username, &password)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;

    // Take WS receivers and MCP session to give to GameStateManager
    let ws_message_rx = session
        .ws_message_rx
        .take()
        .ok_or("WS message receiver missing after connect")?;
    let ws_state_rx = session
        .ws_state_rx
        .take()
        .ok_or("WS state receiver missing after connect")?;

    // Re-take the MCP session from the AccountSession to give to the GSM.
    // The GSM owns it for the duration of the connection.
    //
    // We need to take it out, give it to the GSM, and the GSM will own it.
    // The AccountSession's `mcp_session` field becomes None — but we still
    // need to track the session lifecycle. The GSM task's JoinHandle goes
    // into the AccountSession via `set_gsm_handle()`.
    //
    // Actually, looking at AccountSession more carefully: the MCP session
    // is used by both the GSM (for queries) and for session lifecycle
    // (is_connected, game_session_id). We need to share it.
    //
    // Option A: Move MCP into GSM, and the GSM owns it. AccountSession
    //   tracks connection state via the WS handle only.
    // Option B: Wrap McpSession in Arc<Mutex> and share.
    //
    // Going with Option A — simpler, matches the plan's intent. The GSM
    // is the sole consumer of MCP queries. AccountSession tracks lifecycle
    // via ws_handle + gsm_handle.
    let mcp_session = session
        .mcp_session_take()
        .ok_or("MCP session missing after connect")?;

    // Create and spawn GameStateManager
    let mut gsm = GameStateManager::new(
        app.clone(),
        username.clone(),
        ws_message_rx,
        ws_state_rx,
        mcp_session,
    );
    let gsm_handle = tokio::spawn(async move {
        gsm.start().await;
    });
    session.set_gsm_handle(gsm_handle);

    // Insert into HashMap
    {
        let mut sessions = state.sessions.lock().await;
        sessions.insert(username.clone(), session);
    }

    info!(username, "Account connected and GSM spawned");

    // Wait 2s before releasing the queue lock (inter-account delay)
    tokio::time::sleep(INTER_ACCOUNT_DELAY).await;

    Ok(())
}

/// Disconnect an account. Aborts WS + GSM tasks, shuts down MCP session.
#[tauri::command]
async fn disconnect_account(
    state: State<'_, AppState>,
    username: String,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().await;
    if let Some(mut session) = sessions.remove(&username) {
        session.disconnect().await;
        info!(username, "Account disconnected via command");
        Ok(())
    } else {
        Err(format!("Account '{username}' is not connected"))
    }
}

/// Check if an account is currently connected.
#[tauri::command]
async fn is_account_connected(
    state: State<'_, AppState>,
    username: String,
) -> Result<bool, String> {
    let sessions = state.sessions.lock().await;
    Ok(sessions
        .get(&username)
        .is_some_and(|s| s.is_connected()))
}

/// List currently connected account usernames.
#[tauri::command]
async fn list_connected_accounts(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let sessions = state.sessions.lock().await;
    Ok(sessions.keys().cloned().collect())
}

// === AppState construction ===

/// Create the AppState for the Tauri app.
/// Called from `lib.rs` during app setup.
pub fn create_app_state() -> AppState {
    // Stronghold snapshot path — stored in the app data directory.
    // On Windows: %APPDATA%/SpaceMoltViewer/credentials.stronghold
    // On macOS/Linux: ~/.local/share/SpaceMoltViewer/credentials.stronghold
    let app_data = std::env::var("APPDATA")
        .map(|p| PathBuf::from(p).join("SpaceMoltViewer"))
        .unwrap_or_else(|_| {
            dirs_next::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("SpaceMoltViewer")
        });

    let snapshot_path = app_data.join("credentials.stronghold");
    let config_path = app_data.join("accounts.json");

    // Ensure the directory exists
    let _ = std::fs::create_dir_all(&app_data);

    let credential_store = CredentialStore::new(&snapshot_path);
    let account_manager = AccountManager::new(credential_store, config_path);

    AppState {
        account_manager: Mutex::new(account_manager),
        sessions: Mutex::new(HashMap::new()),
        connect_queue: Mutex::new(()),
    }
}

/// Get the invoke handler for all commands.
/// Must be called from the same module as the `#[tauri::command]` functions
/// so that the generated `__cmd__` macros are visible.
pub fn invoke_handler() -> Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static> {
    Box::new(tauri::generate_handler![
        credentials_initialized,
        init_credentials,
        load_accounts,
        get_accounts,
        add_account,
        remove_account,
        update_password,
        connect_account,
        disconnect_account,
        is_account_connected,
        list_connected_accounts,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_account_delay_is_2_seconds() {
        assert_eq!(INTER_ACCOUNT_DELAY, std::time::Duration::from_secs(2));
    }
}