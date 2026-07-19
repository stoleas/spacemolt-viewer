// SpaceMolt Viewer — MCP client via rmcp SDK + session recovery.
//
// Wraps the official rmcp Rust SDK's StreamableHttpClientTransport with the
// session-recovery pattern from the fleet's Python GameSession._call:
//   - On `session_required` / `session_invalid` error markers, re-login once
//     and retry the original call once (no unbounded retry loop).
//   - rmcp 2.x's built-in `reinit_on_expired_session` handles transport-level
//     session expiry (HTTP 404), but SpaceMolt uses game-level session markers
//     as text in error responses — so we need our own recovery layer on top.
//
// References:
//   - Fleet: ~/Projects/spacemolt/src/spacemolt/game_client/mcp_session.py
//   - rmcp API: src/transport/streamable_http_client.rs (v2.2.0)
//   - Decision #4 (grilling 2026-07-19): use rmcp, not hand-rolled JSON-RPC
//   - Decision #8: WS push-only, MCP for all queries

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Implementation,
};
use rmcp::service::{serve_client, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::Value;
use thiserror::Error;

/// Error markers the SpaceMolt server returns when the game session has
/// expired. On these, re-login and retry once — per the server's own
/// session-recovery contract (confirmed live by the fleet project).
const SESSION_RECOVERY_MARKERS: &[&str] = &["session_required", "session_invalid"];

/// All SpaceMolt MCP traffic goes to this single endpoint.
const MCP_ENDPOINT: &str = "https://game.spacemolt.com/mcp";

/// Errors from the MCP client layer.
#[derive(Error, Debug)]
pub enum McpError {
    /// A non-recoverable error from the MCP server (or a session error that
    /// recovery couldn't resolve).
    #[error("MCP tool '{tool}' failed: {message}")]
    ToolError { tool: String, message: String },

    /// The tool call succeeded at the transport level but the response
    /// content was empty or unparseable.
    #[error("MCP tool '{tool}' returned no parseable content")]
    NoContent { tool: String },

    /// Session recovery failed — re-login attempt itself errored.
    #[error("session recovery failed during '{tool}': {message}")]
    RecoveryFailed { tool: String, message: String },

    /// Transport-level error from rmcp (connection, initialize, etc.).
    #[error("MCP transport error: {0}")]
    Transport(String),

    /// JSON parse error on tool response content.
    #[error("JSON parse error in '{tool}' response: {source}")]
    JsonParse {
        tool: String,
        #[source]
        source: serde_json::Error,
    },
}

/// One MCP session to the SpaceMolt server. Holds:
///   - The rmcp `RunningService` (manages the streamable HTTP transport,
///     JSON-RPC envelopes, mcp-session-id header, SSE framing)
///   - The game-level `session_id` (from `login` tool call, threaded into
///     every subsequent tool call's arguments)
///   - Credentials for re-login on session expiry
///
/// The rmcp `RunningService` has a `DropGuard` that cancels its worker on
/// drop, so no explicit shutdown is needed — but `shutdown()` is provided
/// for graceful close.
///
/// Usage:
///   ```ignore
///   let mut session = McpSession::connect().await?;
///   session.login("username", "password").await?;
///   let status = session.call("get_status").await?;
///   ```
pub struct McpSession {
    /// The running rmcp client service. `None` after `shutdown()`.
    /// Using `ClientInfo` as the handler so we can customize client identity.
    service: Option<RunningService<rmcp::RoleClient, ClientInfo>>,
    /// Game session ID from `login` tool call. Threaded into every
    /// subsequent tool call's arguments.
    session_id: Option<String>,
    /// Credentials retained for re-login on session expiry.
    username: Option<String>,
    password: Option<String>,
}

impl McpSession {
    /// Connect to the SpaceMolt MCP endpoint and complete the MCP
    /// initialize handshake. Does NOT log in — call `login()` next.
    pub async fn connect() -> Result<Self, McpError> {
        Self::connect_to(MCP_ENDPOINT).await
    }

    /// Connect to a custom MCP endpoint (for testing).
    pub async fn connect_to(endpoint: &str) -> Result<Self, McpError> {
        let transport = StreamableHttpClientTransport::from_uri(endpoint);

        // ClientInfo (InitializeRequestParams) impls ClientHandler, so we
        // pass it directly as the service. serve_client() completes the
        // MCP initialize handshake (send InitializeRequest, receive
        // InitializeResult, send InitializedNotification).
        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("spacemolt-viewer", "0.1.0"),
        );

        let service = serve_client(client_info, transport)
            .await
            .map_err(|e| McpError::Transport(format!("initialize failed: {e}")))?;

        tracing::info!(
            endpoint,
            "MCP session connected (transport-level handshake complete)"
        );

        Ok(Self {
            service: Some(service),
            session_id: None,
            username: None,
            password: None,
        })
    }

    /// Log in to the game and store the session_id. Never reuse a prior
    /// session_id after this — per the server's contract, the old one no
    /// longer exists once a new login has happened.
    pub async fn login(&mut self, username: &str, password: &str) -> Result<(), McpError> {
        self.username = Some(username.to_string());
        self.password = Some(password.to_string());

        let mut args = serde_json::Map::new();
        args.insert("username".into(), Value::String(username.into()));
        args.insert("password".into(), Value::String(password.into()));

        let result = self.raw_call("login", args).await?;

        // login returns { "session_id": "..." } in the text content
        let session_id = result
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::ToolError {
                tool: "login".into(),
                message: "login response missing session_id field".into(),
            })?
            .to_string();

        self.session_id = Some(session_id);
        tracing::info!(username, "MCP game session established");
        Ok(())
    }

    /// Call a game tool with session_id auto-injected. On
    /// session_required/session_invalid, re-logs in once and retries once.
    /// This is the public API for all queries after login.
    pub async fn call(&mut self, tool: &str) -> Result<Value, McpError> {
        self.call_with_args(tool, serde_json::Map::new()).await
    }

    /// Call a game tool with extra arguments. session_id is auto-injected
    /// (unless the tool is `login` or `register`). On session expiry
    /// markers, re-logs in once and retries once.
    pub async fn call_with_args(
        &mut self,
        tool: &str,
        mut args: serde_json::Map<String, Value>,
    ) -> Result<Value, McpError> {
        // Inject session_id for all tools except auth tools
        if tool != "login" && tool != "register" {
            if let Some(ref sid) = self.session_id {
                args.insert("session_id".into(), Value::String(sid.clone()));
            }
        }

        match self.raw_call(tool, args.clone()).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // Check if this is a recoverable session expiry
                let can_recover = tool != "login"
                    && tool != "register"
                    && self.username.is_some()
                    && self.password.is_some()
                    && is_session_expiry_error(&e);

                if !can_recover {
                    return Err(e);
                }

                tracing::warn!(
                    tool,
                    error = %e,
                    "session expired mid-call; re-logging in and retrying once"
                );

                // Re-login
                let username = self.username.as_ref().unwrap().clone();
                let password = self.password.as_ref().unwrap().clone();
                self.login(&username, &password).await.map_err(|login_err| {
                    McpError::RecoveryFailed {
                        tool: tool.into(),
                        message: format!("re-login failed: {login_err}"),
                    }
                })?;

                // Retry the original call with fresh session_id
                if let Some(ref sid) = self.session_id {
                    args.insert("session_id".into(), Value::String(sid.clone()));
                }
                self.raw_call(tool, args).await
            }
        }
    }

    /// Low-level tool call — no session recovery, no session_id injection.
    /// Sends the tool call via rmcp, extracts text content, parses as JSON.
    async fn raw_call(
        &self,
        tool: &str,
        args: serde_json::Map<String, Value>,
    ) -> Result<Value, McpError> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| McpError::Transport("session has been shut down".into()))?;

        let params = CallToolRequestParams::new(tool.to_string()).with_arguments(args);

        let result: CallToolResult = service
            .call_tool(params)
            .await
            .map_err(|e| McpError::Transport(format!("call_tool '{tool}': {e}")))?;

        // Check for error responses — SpaceMolt returns errors as text
        // content with is_error = true. The error message is in content[0].text.
        if result.is_error.unwrap_or(false) {
            let message = result
                .content
                .first()
                .and_then(|block| block.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(McpError::ToolError {
                tool: tool.into(),
                message,
            });
        }

        // Try structured_content first (if server provides it)
        if let Some(structured) = result.structured_content {
            if structured.is_object() {
                return Ok(structured);
            }
        }

        // Fall back to parsing text content as JSON
        let text = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|t| t.text.as_str())
            .ok_or_else(|| McpError::NoContent {
                tool: tool.into(),
            })?;

        serde_json::from_str::<Value>(text).map_err(|source| McpError::JsonParse {
            tool: tool.into(),
            source,
        })
    }

    /// Gracefully shut down the MCP session. Cancels the rmcp worker and
    /// waits for it to complete. The session is unusable after this.
    pub async fn shutdown(&mut self) {
        if let Some(service) = self.service.take() {
            // cancel() consumes the service and waits for the worker to stop
            let _ = service.cancel().await;
        }
        self.session_id = None;
    }

    /// Current game session ID (for diagnostics / state display).
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Whether the session is connected and logged in.
    pub fn is_active(&self) -> bool {
        self.service.is_some() && self.session_id.is_some()
    }
}

/// Check if an McpError is a session-expiry marker that warrants recovery.
fn is_session_expiry_error(err: &McpError) -> bool {
    let message = match err {
        McpError::ToolError { message, .. } => message,
        McpError::Transport(msg) => msg,
        _ => return false,
    };
    SESSION_RECOVERY_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_expiry_detection() {
        assert!(is_session_expiry_error(&McpError::ToolError {
            tool: "get_status".into(),
            message: "session_required: please login again".into(),
        }));

        assert!(is_session_expiry_error(&McpError::ToolError {
            tool: "get_nearby".into(),
            message: "session_invalid".into(),
        }));

        assert!(!is_session_expiry_error(&McpError::ToolError {
            tool: "get_status".into(),
            message: "some other error".into(),
        }));

        assert!(!is_session_expiry_error(&McpError::NoContent {
            tool: "get_status".into(),
        }));
    }

    #[test]
    fn test_session_recovery_markers_complete() {
        // These are the exact markers the fleet project identified
        // from the server's own session-recovery contract.
        assert_eq!(
            SESSION_RECOVERY_MARKERS,
            &["session_required", "session_invalid"]
        );
    }

    #[test]
    fn test_mcp_error_display() {
        let err = McpError::ToolError {
            tool: "get_status".into(),
            message: "session_required".into(),
        };
        assert!(err.to_string().contains("get_status"));
        assert!(err.to_string().contains("session_required"));
    }
}