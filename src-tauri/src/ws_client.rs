// SpaceMolt Viewer — WebSocket v2 client with close-code-aware auto-reconnect.
//
// The WS connection is push-only (Decision #8: WS v2 for push events, MCP for
// all queries). The client never sends anything except the login handshake.
// Every inbound frame is either auth (welcome/logged_in) or a push event —
// routed by `type` and forwarded to the caller via an mpsc channel.
//
// Close-code-aware reconnect state machine (Decision from grilling 2026-07-19):
//   1000 = normal close → exponential backoff reconnect
//   4001 = session_replaced → ABORT (do NOT reconnect — a new connection
//          displaced this one; blind backoff creates an infinite reconnect loop)
//   4002 = auth_timeout → immediate reconnect (no delay)
//   4003 = rate_limited → parse `retry_after` from close reason, wait that many
//          seconds, then reconnect (blind backoff may reconnect before the
//          cooldown expires, escalating to an IP block)
//
// WS v2 endpoint: wss://game.spacemolt.com/ws/v2
// v2 framing: {"tool":"<tool>","action":"<action>","payload":{...},"request_id":"..."}
// Auth frames (welcome/logged_in) are identical on v1 and v2.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async,
    tungstenite::protocol::frame::coding::CloseCode,
    tungstenite::Message,
    WebSocketStream, MaybeTlsStream,
};
use tracing::{error, info, warn};

use crate::models::ConnectionState;

/// WS v2 endpoint — push events only (Decision #8).
pub const WS_URL: &str = "wss://game.spacemolt.com/ws/v2";

/// Maximum reconnect attempts before giving up.
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Maximum backoff delay between reconnect attempts (caps exponential growth).
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Timeout for receiving the `welcome` frame after connecting.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for receiving the `logged_in` frame after sending login.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);

/// A raw WS message — the `type` field plus the unparsed `payload` JSON.
/// The caller (GameStateManager) routes by `msg_type` and deserializes the
/// payload into the appropriate models.rs struct.
#[derive(Debug, Clone)]
pub struct WsRawMessage {
    pub msg_type: String,
    pub payload: Value,
}

/// What the reconnect state machine should do when the WS closes.
/// Derived from the close code per the grilling decisions (2026-07-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectAction {
    /// Code 1000 (normal close) — exponential backoff reconnect.
    Backoff { delay: Duration, attempt: u32 },
    /// Code 4001 (session_replaced) — do NOT reconnect. A new connection
    /// displaced this one; reconnecting would fight the replacement.
    Abort,
    /// Code 4002 (auth_timeout) — reconnect immediately, no delay.
    Immediate,
    /// Code 4003 (rate_limited) — wait `retry_after` seconds, then reconnect.
    WaitFor { seconds: u64 },
}

/// Classify a close code into the appropriate reconnect action.
///
/// This is the heart of the close-code-aware state machine. The function is
/// pure and tested in isolation — no network needed.
pub fn classify_close_code(code: u16, reason: &str, attempt: u32) -> ReconnectAction {
    match code {
        4001 => ReconnectAction::Abort,
        4002 => ReconnectAction::Immediate,
        4003 => {
            // Parse retry_after from the close reason string.
            // Server sends it as part of the close reason text.
            // Try JSON parse first (server may send structured reason),
            // then fall back to scanning for a number.
            let seconds = parse_retry_after(reason).unwrap_or(30);
            ReconnectAction::WaitFor { seconds }
        }
        _ => {
            // 1000 (normal) or any other code — exponential backoff.
            let exp = (attempt - 1).min(5) as u32; // cap exponent at 5
            let delay = Duration::from_secs(1u64 << exp).min(MAX_BACKOFF);
            ReconnectAction::Backoff { delay, attempt }
        }
    }
}

/// Parse `retry_after` from a close reason string.
///
/// The server may send the close reason as a JSON string like
/// `{"retry_after": 60}` or as plain text containing a number.
/// Returns None if no number is found.
fn parse_retry_after(reason: &str) -> Option<u64> {
    // Try JSON parse first.
    if let Ok(json) = serde_json::from_str::<Value>(reason) {
        if let Some(retry) = json.get("retry_after").and_then(|v| v.as_u64()) {
            return Some(retry);
        }
    }
    // Fall back: scan for a number in the reason string.
    // Look for patterns like "retry_after": 60 or retry_after=60 or just "60".
    let trimmed = reason.trim();
    if let Ok(n) = trimmed.parse::<u64>() {
        return Some(n);
    }
    // Try to find a number after "retry_after" in any format.
    if let Some(pos) = reason.to_lowercase().find("retry_after") {
        let after = &reason[pos + 11..];
        let num_str: String = after.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<u64>() {
            return Some(n);
        }
    }
    None
}

/// Parse a close code from a tungstenite CloseFrame.
/// tungstenite represents custom codes (4000-4999) as `CloseCode::Library(u16)`,
/// and known codes as named variants. We extract the raw u16 in all cases.
fn close_code_to_u16(code: &CloseCode) -> u16 {
    code.into()
}

/// WS v2 client. Push-only — receives server events and forwards them via
/// channels. The caller owns the `message_rx` and `state_rx` receivers.
///
/// Usage:
/// ```ignore
/// let (message_tx, message_rx) = mpsc::channel(100);
/// let (state_tx, state_rx) = mpsc::channel(20);
/// let client = WsClient::new(state_tx, message_tx);
/// let (logged_in, ws_join) = client.connect_and_login("user", "pass").await?;
/// // message_rx and state_rx now receive push events and connection state changes
/// // ws_join can be .abort()'d to stop the background receive loop
/// ```
pub struct WsClient {
    state_tx: mpsc::Sender<ConnectionState>,
    message_tx: mpsc::Sender<WsRawMessage>,
}

/// Internal handle held by the receive loop so it can reconnect with the
/// same credentials. Cloned from WsClient before spawning the loop.
struct WsLoopHandle {
    message_tx: mpsc::Sender<WsRawMessage>,
    state_tx: mpsc::Sender<ConnectionState>,
    username: String,
    password: String,
}

impl WsClient {
    pub fn new(state_tx: mpsc::Sender<ConnectionState>, message_tx: mpsc::Sender<WsRawMessage>) -> Self {
        Self { state_tx, message_tx }
    }

    /// Connect to the WS v2 endpoint, perform the login handshake, and spawn
    /// the background receive loop. Returns the `logged_in` payload (initial
    /// full state snapshot) and the `JoinHandle` for the spawned receive loop
    /// so the caller (`AccountSession`) can `abort()` it on disconnect.
    pub async fn connect_and_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(Value, JoinHandle<()>), String> {
        let _ = self.state_tx.send(ConnectionState::Connecting).await;

        let (ws_stream, _response) =
            connect_async(WS_URL).await.map_err(|e| format!("WebSocket connect failed: {e}"))?;

        let mut ws_stream = ws_stream;

        // Wait for welcome frame.
        let _welcome = wait_for_frame(&mut ws_stream, "welcome", WELCOME_TIMEOUT).await?;

        // Send login (v2 framing: tool/action, not flat type).
        let login_msg = json!({
            "tool": "spacemolt_auth",
            "action": "login",
            "payload": {"username": username, "password": password}
        });
        ws_stream
            .send(Message::Text(login_msg.to_string()))
            .await
            .map_err(|e| format!("Send login failed: {e}"))?;

        // Wait for logged_in frame (initial state snapshot).
        let logged_in = wait_for_frame(&mut ws_stream, "logged_in", LOGIN_TIMEOUT).await?;

        let _ = self.state_tx.send(ConnectionState::Connected).await;

        // Spawn the receive loop in the background.
        let handle = WsLoopHandle {
            message_tx: self.message_tx.clone(),
            state_tx: self.state_tx.clone(),
            username: username.to_string(),
            password: password.to_string(),
        };
        let join_handle = tokio::spawn(receive_loop(ws_stream, handle));

        Ok((logged_in, join_handle))
    }
}

/// Wait for a specific frame type from the WS stream. Returns the payload.
/// Ignores non-matching text frames (could be late push events during handshake).
async fn wait_for_frame(
    ws_stream: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    expected_type: &str,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let msg = tokio::time::timeout_at(deadline, ws_stream.next())
            .await
            .map_err(|_| format!("Timed out waiting for {expected_type}"))?
            .ok_or("WebSocket stream ended")?
            .map_err(|e| format!("WebSocket receive error: {e}"))?;

        if let Message::Text(text) = msg {
            let json: Value = serde_json::from_str(&text)
                .map_err(|e| format!("JSON parse error: {e}"))?;

            let msg_type = json.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let payload = json.get("payload").cloned().unwrap_or(json.clone());

            if msg_type == expected_type {
                return Ok(payload);
            }
            // Non-matching frame during handshake — ignore (could be a late push event).
            info!("Ignoring non-matching frame during handshake: type={msg_type} (expected {expected_type})");
        }
        // Ignore Ping/Pong/Close during handshake — tungstenite auto-responds to pings.
    }
}

/// Background receive loop — reads frames until the stream closes or errors,
/// then applies the close-code-aware reconnect state machine.
async fn receive_loop(mut ws_stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, handle: WsLoopHandle) {
    let mut attempt: u32 = 0;

    loop {
        match ws_stream.next().await {
            // Text frame — parse and forward to the caller.
            Some(Ok(Message::Text(text))) => {
                if let Ok(json) = serde_json::from_str::<Value>(&text) {
                    let msg_type = json
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let payload = json.get("payload").cloned().unwrap_or(json.clone());

                    let msg = WsRawMessage { msg_type, payload };
                    if handle.message_tx.send(msg).await.is_err() {
                        info!("WS receive loop: message receiver dropped, stopping");
                        break;
                    }
                } else {
                    warn!("WS receive loop: failed to parse text frame as JSON");
                }
            }

            // Close frame — extract close code and apply reconnect state machine.
            Some(Ok(Message::Close(Some(close_frame)))) => {
                let code = close_code_to_u16(&close_frame.code);
                let reason = close_frame.reason.as_ref().to_string();
                info!(code, reason = %reason, "WebSocket closed");

                let action = classify_close_code(code, &reason, attempt + 1);
                match action {
                    ReconnectAction::Abort => {
                        info!("WS close code 4001 (session_replaced) — aborting, no reconnect");
                        let _ = handle.state_tx.send(ConnectionState::SessionReplaced).await;
                        break;
                    }
                    ReconnectAction::WaitFor { seconds } => {
                        warn!(seconds, "WS close code 4003 (rate_limited) — waiting before reconnect");
                        let _ = handle
                            .state_tx
                            .send(ConnectionState::RateLimited { retry_after: seconds })
                            .await;
                        tokio::time::sleep(Duration::from_secs(seconds)).await;
                    }
                    ReconnectAction::Backoff { delay, attempt: new_attempt } => {
                        info!(delay_ms = delay.as_millis(), attempt = new_attempt, "WS close code 1000 — backoff reconnect");
                        let _ = handle.state_tx.send(ConnectionState::Reconnecting).await;
                        tokio::time::sleep(delay).await;
                        attempt = new_attempt;
                    }
                    ReconnectAction::Immediate => {
                        info!("WS close code 4002 (auth_timeout) — immediate reconnect");
                        let _ = handle.state_tx.send(ConnectionState::Reconnecting).await;
                        // Reset attempt counter for immediate reconnect.
                        attempt = 0;
                    }
                }

                // Attempt to reconnect.
                match reconnect_with_login(&handle, attempt).await {
                    Ok(new_stream) => {
                        ws_stream = new_stream;
                        let _ = handle.state_tx.send(ConnectionState::Connected).await;
                        info!("WS reconnected successfully");
                        // Continue the receive loop with the new stream.
                    }
                    Err(e) => {
                        error!("WS reconnection failed after {attempt} attempts: {e}");
                        let _ = handle.state_tx.send(ConnectionState::Error).await;
                        break;
                    }
                }
            }

            // Stream ended without a close frame (abrupt disconnect).
            Some(Ok(Message::Close(None))) | None => {
                info!("WebSocket stream ended without close frame");
                let _ = handle.state_tx.send(ConnectionState::Reconnecting).await;
                attempt += 1;
                let delay = Duration::from_secs(1u64 << (attempt - 1).min(5)).min(MAX_BACKOFF);
                tokio::time::sleep(delay).await;

                match reconnect_with_login(&handle, attempt).await {
                    Ok(new_stream) => {
                        ws_stream = new_stream;
                        let _ = handle.state_tx.send(ConnectionState::Connected).await;
                        info!("WS reconnected after abrupt disconnect");
                    }
                    Err(e) => {
                        error!("WS reconnection failed: {e}");
                        let _ = handle.state_tx.send(ConnectionState::Error).await;
                        break;
                    }
                }
            }

            // Ping/Pong — tungstenite auto-responds to pings, so we just log and continue.
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                // Tungstenite handles ping/pong automatically. No action needed.
            }

            // Binary frames — SpaceMolt only sends text, but be resilient.
            Some(Ok(Message::Binary(_))) => {
                warn!("WS received unexpected binary frame, ignoring");
            }

            // Error from the stream — treat as disconnect, attempt reconnect.
            Some(Err(e)) => {
                error!("WebSocket stream error: {e}");
                let _ = handle.state_tx.send(ConnectionState::Reconnecting).await;
                attempt += 1;
                let delay = Duration::from_secs(1u64 << (attempt - 1).min(5)).min(MAX_BACKOFF);
                tokio::time::sleep(delay).await;

                match reconnect_with_login(&handle, attempt).await {
                    Ok(new_stream) => {
                        ws_stream = new_stream;
                        let _ = handle.state_tx.send(ConnectionState::Connected).await;
                        info!("WS reconnected after stream error");
                    }
                    Err(e) => {
                        error!("WS reconnection failed after stream error: {e}");
                        let _ = handle.state_tx.send(ConnectionState::Error).await;
                        break;
                    }
                }
            }

            // Raw Frame — tungstenite docs say this won't appear in reads.
            Some(Ok(Message::Frame(_))) => {
                // Should not happen per tungstenite docs.
            }
        }
    }
}

/// Reconnect to the WS v2 endpoint and perform the login handshake.
/// Returns the new WebSocketStream on success.
async fn reconnect_with_login(
    handle: &WsLoopHandle,
    attempt: u32,
) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, String> {
    if attempt > MAX_RECONNECT_ATTEMPTS {
        return Err(format!("Reconnection failed after {MAX_RECONNECT_ATTEMPTS} attempts"));
    }

    info!(attempt, "Attempting WS reconnect");

    let (ws_stream, _) = connect_async(WS_URL)
        .await
        .map_err(|e| format!("Reconnect failed: {e}"))?;

    let mut ws_stream = ws_stream;

    // Wait for welcome.
    let _welcome = wait_for_frame(&mut ws_stream, "welcome", WELCOME_TIMEOUT)
        .await
        .map_err(|e| format!("Reconnect: {e}"))?;

    // Send login.
    let login_msg = json!({
        "tool": "spacemolt_auth",
        "action": "login",
        "payload": {"username": handle.username, "password": handle.password}
    });
    ws_stream
        .send(Message::Text(login_msg.to_string()))
        .await
        .map_err(|e| format!("Reconnect login send failed: {e}"))?;

    // Wait for logged_in.
    let _logged_in = wait_for_frame(&mut ws_stream, "logged_in", LOGIN_TIMEOUT)
        .await
        .map_err(|e| format!("Reconnect: {e}"))?;

    Ok(ws_stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Reconnect action classification ===

    #[test]
    fn close_code_1000_returns_backoff() {
        let action = classify_close_code(1000, "normal closure", 1);
        assert_eq!(action, ReconnectAction::Backoff { delay: Duration::from_secs(1), attempt: 1 });
    }

    #[test]
    fn close_code_1000_exponential_backoff() {
        let a1 = classify_close_code(1000, "", 1);
        let a2 = classify_close_code(1000, "", 2);
        let a3 = classify_close_code(1000, "", 3);
        assert_eq!(a1, ReconnectAction::Backoff { delay: Duration::from_secs(1), attempt: 1 });
        assert_eq!(a2, ReconnectAction::Backoff { delay: Duration::from_secs(2), attempt: 2 });
        assert_eq!(a3, ReconnectAction::Backoff { delay: Duration::from_secs(4), attempt: 3 });
    }

    #[test]
    fn close_code_1000_backoff_caps_at_30s() {
        let action = classify_close_code(1000, "", 10);
        // 1 << 9 = 512s, but capped at 30s
        assert_eq!(action, ReconnectAction::Backoff { delay: MAX_BACKOFF, attempt: 10 });
    }

    #[test]
    fn close_code_4001_returns_abort() {
        let action = classify_close_code(4001, "session_replaced", 1);
        assert_eq!(action, ReconnectAction::Abort);
    }

    #[test]
    fn close_code_4002_returns_immediate() {
        let action = classify_close_code(4002, "auth_timeout", 1);
        assert_eq!(action, ReconnectAction::Immediate);
    }

    #[test]
    fn close_code_4003_parses_json_retry_after() {
        let reason = r#"{"retry_after": 60}"#;
        let action = classify_close_code(4003, reason, 1);
        assert_eq!(action, ReconnectAction::WaitFor { seconds: 60 });
    }

    #[test]
    fn close_code_4003_parses_plain_number() {
        let action = classify_close_code(4003, "45", 1);
        assert_eq!(action, ReconnectAction::WaitFor { seconds: 45 });
    }

    #[test]
    fn close_code_4003_parses_retry_after_in_text() {
        let action = classify_close_code(4003, "rate limited, retry_after=120 seconds", 1);
        assert_eq!(action, ReconnectAction::WaitFor { seconds: 120 });
    }

    #[test]
    fn close_code_4003_defaults_to_30_when_no_number() {
        let action = classify_close_code(4003, "rate limited", 1);
        assert_eq!(action, ReconnectAction::WaitFor { seconds: 30 });
    }

    // === retry_after parsing ===

    #[test]
    fn parse_retry_after_json_object() {
        let result = parse_retry_after(r#"{"retry_after": 90}"#);
        assert_eq!(result, Some(90));
    }

    #[test]
    fn parse_retry_after_plain_number() {
        let result = parse_retry_after("30");
        assert_eq!(result, Some(30));
    }

    #[test]
    fn parse_retry_after_text_with_number() {
        let result = parse_retry_after("retry_after: 15");
        assert_eq!(result, Some(15));
    }

    #[test]
    fn parse_retry_after_no_number_returns_none() {
        let result = parse_retry_after("rate limited, try again later");
        assert_eq!(result, None);
    }

    #[test]
    fn parse_retry_after_empty_string() {
        let result = parse_retry_after("");
        assert_eq!(result, None);
    }

    // === CloseFrame to u16 conversion ===

    #[test]
    fn close_code_to_u16_normal() {
        assert_eq!(close_code_to_u16(&CloseCode::Normal), 1000);
    }

    #[test]
    fn close_code_to_u16_library_custom() {
        // SpaceMolt custom codes (4001, 4002, 4003) are in the 4000-4999 range,
        // which tungstenite represents as CloseCode::Library(u16).
        assert_eq!(close_code_to_u16(&CloseCode::Library(4001)), 4001);
        assert_eq!(close_code_to_u16(&CloseCode::Library(4002)), 4002);
        assert_eq!(close_code_to_u16(&CloseCode::Library(4003)), 4003);
    }

    // === WsRawMessage ===

    #[test]
    fn ws_raw_message_construction() {
        let payload = json!({"tick": 42, "player": {"username": "test"}});
        let msg = WsRawMessage {
            msg_type: "state_update".to_string(),
            payload: payload.clone(),
        };
        assert_eq!(msg.msg_type, "state_update");
        assert_eq!(msg.payload, payload);
    }

    // === ReconnectAction equality ===

    #[test]
    fn reconnect_action_abort_is_distinct() {
        assert_ne!(ReconnectAction::Abort, ReconnectAction::Immediate);
        assert_ne!(
            ReconnectAction::Abort,
            ReconnectAction::Backoff { delay: Duration::from_secs(1), attempt: 1 }
        );
    }

    #[test]
    fn reconnect_action_wait_for_equality() {
        assert_eq!(
            ReconnectAction::WaitFor { seconds: 30 },
            ReconnectAction::WaitFor { seconds: 30 }
        );
        assert_ne!(
            ReconnectAction::WaitFor { seconds: 30 },
            ReconnectAction::WaitFor { seconds: 60 }
        );
    }

    // === Constants ===

    #[test]
    fn max_reconnect_attempts_is_reasonable() {
        // 10 attempts gives enough retries for transient failures
        // without infinite looping on persistent problems.
        assert_eq!(MAX_RECONNECT_ATTEMPTS, 10);
    }

    #[test]
    fn max_backoff_is_30s() {
        assert_eq!(MAX_BACKOFF, Duration::from_secs(30));
    }

    #[test]
    fn ws_url_is_v2_endpoint() {
        assert_eq!(WS_URL, "wss://game.spacemolt.com/ws/v2");
    }
}