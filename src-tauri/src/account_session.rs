// SpaceMolt Viewer — Per-account connection lifecycle (Decision #5).
//
// Replaces the plan's `SessionManager` single-struct with `AccountSession`,
// stored in a `HashMap<String, AccountSession>` keyed by username.
//
// `AccountSession` owns:
//   - `McpSession` (rmcp transport + game session_id)
//   - WS receive-loop `JoinHandle` (from `WsClient::connect_and_login`)
//   - GSM `JoinHandle` (Task 9 — slot reserved, populated when GSM is implemented)
//   - WS event receivers (`ws_message_rx`, `ws_state_rx`)
//
// On disconnect:
//   1. `abort()` the WS `JoinHandle` (stops background receive loop)
//   2. `abort()` the GSM `JoinHandle` if present
//   3. `shutdown()` the `McpSession` (closes MCP transport via rmcp DropGuard)
//   4. Drop the receivers (closes channels)
//
// Per Decision #10: the HashMap lock is held only for insert/remove (microseconds),
// never during connect/login. `connect()` is called BEFORE inserting the
// `AccountSession` into the HashMap.
//
// References:
//   - Decision #5: AccountSession replaces SessionManager
//   - Decision #8: WS push-only, MCP for all queries
//   - Decision #10: Serial connection staggering
//   - Plan Task 8 (superseded by Decision #5)

use crate::mcp_client::McpSession;
use crate::models::ConnectionState;
use crate::ws_client::{WsClient, WsRawMessage};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::info;

/// One account's connection lifecycle. Owns the MCP session, WS receive
/// loop handle, and WS event receivers.
///
/// Usage:
///   ```ignore
///   // Connect BEFORE inserting into the HashMap (lock held only for insert)
///   let mut session = AccountSession::new();
///   session.connect("user", "pass").await?;
///   sessions.lock().await.insert("user".to_string(), session);
///
///   // Later — disconnect and remove
///   let mut session = sessions.lock().await.remove("user").unwrap();
///   session.disconnect().await;
///   ```
pub struct AccountSession {
    /// MCP session — owns the rmcp transport + game session_id.
    /// `None` when disconnected.
    mcp_session: Option<McpSession>,

    /// WS background receive-loop handle. `abort()` on disconnect.
    ws_handle: Option<JoinHandle<()>>,

    /// GameStateManager background task handle (Task 9 — not yet implemented).
    /// `abort()` on disconnect. Will be `Some` once GSM is wired in.
    gsm_handle: Option<JoinHandle<()>>,

    /// Receiver for WS push events (messages from server).
    /// Taken out and given to GameStateManager when GSM starts.
    pub ws_message_rx: Option<mpsc::Receiver<WsRawMessage>>,

    /// Receiver for WS connection state changes.
    /// Taken out and given to GameStateManager when GSM starts.
    pub ws_state_rx: Option<mpsc::Receiver<ConnectionState>>,
}

impl AccountSession {
    /// Create a new disconnected session (no active connections).
    pub fn new() -> Self {
        Self {
            mcp_session: None,
            ws_handle: None,
            gsm_handle: None,
            ws_message_rx: None,
            ws_state_rx: None,
        }
    }

    /// Connect this account: WS connect+login, then MCP connect+login.
    ///
    /// This is meant to be called BEFORE inserting into the HashMap,
    /// so the lock isn't held during network operations (Decision #10).
    pub async fn connect(&mut self, username: &str, password: &str) -> Result<(), String> {
        info!(username, "Starting connection for account");

        // Create channels for WebSocket events
        let (state_tx, state_rx) = mpsc::channel(100);
        let (message_tx, message_rx) = mpsc::channel(500);

        // Connect WebSocket (push-only, Decision #8)
        let ws_client = WsClient::new(state_tx, message_tx);
        let (_logged_in, ws_join) = ws_client
            .connect_and_login(username, password)
            .await
            .map_err(|e| format!("WebSocket connection failed: {e}"))?;

        self.ws_handle = Some(ws_join);
        self.ws_message_rx = Some(message_rx);
        self.ws_state_rx = Some(state_rx);

        // Connect MCP and login
        let mut mcp = McpSession::connect()
            .await
            .map_err(|e| format!("MCP initialize failed: {e}"))?;
        mcp.login(username, password)
            .await
            .map_err(|e| format!("MCP login failed: {e}"))?;

        self.mcp_session = Some(mcp);

        info!(username, "Account session connected (WS + MCP)");
        Ok(())
    }

    /// Disconnect this account: abort WS and GSM tasks, shutdown MCP session.
    /// Safe to call multiple times (idempotent — all fields are `take()`n).
    pub async fn disconnect(&mut self) {
        info!("Disconnecting account session");

        // Abort WS receive loop
        if let Some(handle) = self.ws_handle.take() {
            handle.abort();
        }

        // Abort GSM task (if present — Task 9 not implemented yet)
        if let Some(handle) = self.gsm_handle.take() {
            handle.abort();
        }

        // Shutdown MCP session (closes transport via rmcp DropGuard)
        if let Some(mut mcp) = self.mcp_session.take() {
            mcp.shutdown().await;
        }

        // Drop receivers (closes channels)
        self.ws_message_rx.take();
        self.ws_state_rx.take();
    }

    /// Whether this session is connected and logged in.
    pub fn is_connected(&self) -> bool {
        self.mcp_session.as_ref().is_some_and(|m| m.is_active())
    }

    /// Get a mutable reference to the MCP session (for GameApi queries).
    /// Returns `None` if disconnected.
    pub fn mcp_session_mut(&mut self) -> Option<&mut McpSession> {
        self.mcp_session.as_mut()
    }

    /// Get the game session ID (for diagnostics / state display).
    pub fn game_session_id(&self) -> Option<&str> {
        self.mcp_session.as_ref().and_then(|m| m.session_id())
    }

    /// Set the GSM JoinHandle (called by Task 9 when GameStateManager starts).
    /// The handle will be aborted on `disconnect()`.
    pub fn set_gsm_handle(&mut self, handle: JoinHandle<()>) {
        self.gsm_handle = Some(handle);
    }
}

impl Default for AccountSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn account_session_starts_disconnected() {
        let session = AccountSession::new();
        assert!(!session.is_connected());
        assert!(session.game_session_id().is_none());
        assert!(session.ws_handle.is_none());
        assert!(session.ws_message_rx.is_none());
        assert!(session.ws_state_rx.is_none());
        assert!(session.gsm_handle.is_none());
    }

    #[tokio::test]
    async fn disconnect_is_idempotent() {
        let mut session = AccountSession::new();
        // Disconnect on a fresh session should not panic
        session.disconnect().await;
        // Second disconnect should also be fine
        session.disconnect().await;
        assert!(!session.is_connected());
    }

    #[tokio::test]
    async fn mcp_session_mut_returns_none_when_disconnected() {
        let mut session = AccountSession::new();
        assert!(session.mcp_session_mut().is_none());
    }

    #[tokio::test]
    async fn set_gsm_handle_stores_handle() {
        let mut session = AccountSession::new();
        let (tx, _rx) = mpsc::channel::<()>(1);
        // Spawn a dummy task that immediately completes
        let handle = tokio::spawn(async move {
            let _ = tx;
        });
        session.set_gsm_handle(handle);
        assert!(session.gsm_handle.is_some());
        // disconnect should abort it
        session.disconnect().await;
        assert!(session.gsm_handle.is_none());
    }
}