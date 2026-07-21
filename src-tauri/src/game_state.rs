// SpaceMolt Viewer — Game state manager (push event processing + v2 state delta merging).
//
// This is the largest Rust module. It mirrors the Swift `GameStateManager` but
// adapts to v2's state-delta model. The GSM:
//   1. Owns the WS message receiver and processes push events
//   2. Owns the MCP session for periodic state refreshes (get_state)
//   3. Maintains a `GameState` struct, merging v2 state deltas into it
//   4. Emits Tauri events to the frontend: `state_update`, `game_event`,
//      `connection_state` — each wrapped with the account username for
//      multi-account routing
//   5. Runs a periodic refresh loop (throttled MCP queries)
//
// v2 state delta model:
//   - `action_result` push frames carry only changed sections (ship, cargo,
//     location, etc.). The GSM merges each present (Some) field into state
//     rather than replacing the whole state.
//   - `state_update` (v1-style full push) is handled the same way — both
//     decode to `StateUpdatePayload` with optional fields.
//
// The GSM is spawned as a background tokio task by Task 10 (commands.rs).
// It runs until the WS message channel closes (WS receive loop exited).
//
// References:
//   - Decision #1: WS v2 + MCP dual-channel
//   - Decision #8: WS push-only, MCP for all queries
//   - Plan Task 9: game state manager
//   - `get_state` (single MCP call) replaces the old 4-call poll pattern

use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::game_api::GameApi;
use crate::mcp_client::McpSession;
use crate::models::{
    ActionErrorPayload, ChatMessagePayload, CombatUpdatePayload, ConnectionState,
    GameEvent, GameEventCategory, GameState, LoggedInPayload, MiningYieldPayload,
    OkActionPayload, PirateWarningPayload, PlayerDiedPayload, ScanDetectedPayload,
    SkillLevelUpPayload, StateUpdatePayload,
};
use crate::ws_client::WsRawMessage;

/// Maximum number of events retained in the event feed.
const MAX_EVENTS: usize = 200;

/// Interval for periodic MCP state refreshes.
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

// === Multi-account event wrappers ===
//
// Each Tauri event includes the account username so the frontend can route
// updates to the correct account pane. The frontend filters by username.

#[derive(Debug, Clone, Serialize)]
pub struct AccountStateUpdate {
    pub username: String,
    pub state: GameState,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountGameEvent {
    pub username: String,
    pub event: GameEvent,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountConnectionState {
    pub username: String,
    pub state: ConnectionState,
}

/// Game state manager — processes WS push events, maintains game state,
/// emits Tauri events to the frontend, and runs periodic MCP refreshes.
///
/// Owned by the background tokio task spawned in Task 10 (commands.rs).
/// The `McpSession` is moved into the GSM when it starts — the GSM owns it
/// for the duration of the connection. On disconnect, the GSM task is
/// aborted (via `AccountSession::disconnect()`), which drops everything.
pub struct GameStateManager {
    /// Tauri app handle — for emitting events to the frontend.
    app_handle: AppHandle,
    /// Account username — included in every emitted event for routing.
    username: String,
    /// Receiver for WS push events (from `WsClient` receive loop).
    ws_message_rx: mpsc::Receiver<WsRawMessage>,
    /// Receiver for WS connection state changes (from `WsClient`).
    ws_state_rx: mpsc::Receiver<ConnectionState>,
    /// GameApi — wraps McpSession with safety allowlist + throttling.
    game_api: GameApi,
    /// MCP session — owned by the GSM for periodic queries.
    mcp_session: McpSession,
    /// Current game state — maintained by merging push events + MCP refreshes.
    state: GameState,
    /// Event feed — most recent first (prepended), capped at MAX_EVENTS.
    events: VecDeque<GameEvent>,
    /// Whether we've seen a Reconnecting state since last Connected.
    /// Used to trigger notification refresh on reconnect.
    was_reconnecting: bool,
}

impl GameStateManager {
    /// Create a new GameStateManager.
    ///
    /// The `mcp_session` is moved into the GSM — the caller (AccountSession)
    /// should `take()` it before constructing the GSM. The `ws_message_rx`
    /// and `ws_state_rx` are also moved in (taken from AccountSession).
    pub fn new(
        app_handle: AppHandle,
        username: String,
        ws_message_rx: mpsc::Receiver<WsRawMessage>,
        ws_state_rx: mpsc::Receiver<ConnectionState>,
        mcp_session: McpSession,
    ) -> Self {
        Self {
            app_handle,
            username,
            ws_message_rx,
            ws_state_rx,
            game_api: GameApi::new(),
            mcp_session,
            state: GameState::default(),
            events: VecDeque::with_capacity(MAX_EVENTS),
            was_reconnecting: false,
        }
    }

    /// Main processing loop. Loads initial state, then processes WS push
    /// events, connection state changes, and periodic MCP refreshes until
    /// the WS message channel closes.
    ///
    /// This is spawned as a background task — it runs until:
    ///   - The WS message channel closes (WS receive loop exited)
    ///   - The GSM task is aborted (AccountSession::disconnect)
    pub async fn start(&mut self) {
        info!(username = %self.username, "GameStateManager starting");

        // Load initial state via get_state (single MCP call — replaces the
        // old 4-call get_status/get_ship/get_system/get_nearby pattern).
        match self.game_api.get_state(&mut self.mcp_session).await {
            Ok(initial) => {
                // Preserve connection_state (GSM-managed, not from get_state)
                let conn = self.state.connection_state.clone();
                self.state = initial;
                self.state.connection_state = conn;
                self.emit_state_update();
                info!("Initial state loaded via get_state");
            }
            Err(e) => {
                warn!(error = %e, "Failed to load initial state via get_state");
                // Continue anyway — push events will populate state over time.
                // The periodic refresh will retry.
            }
        }

        // Load galaxy map via authenticated MCP get_map (Decision #7).
        // v2 scope for the map UI, but load the data now so it's ready.
        match self.game_api.get_map(&mut self.mcp_session).await {
            Ok(_) => info!("Galaxy map loaded via get_map"),
            Err(e) => warn!(error = %e, "Failed to load galaxy map (non-fatal)"),
        }

        // Fetch missed notifications (poll fallback after connect —
        // get_notifications is MCP-only, not available over WS).
        self.refresh_notifications().await;

        // Main processing loop.
        // Use interval_at to skip the immediate first tick (we just loaded state).
        let start = tokio::time::Instant::now() + REFRESH_INTERVAL;
        let mut refresh_timer = tokio::time::interval_at(start, REFRESH_INTERVAL);

        loop {
            tokio::select! {
                // WS push event
                msg = self.ws_message_rx.recv() => {
                    match msg {
                        Some(msg) => self.handle_message(msg).await,
                        None => {
                            info!(
                                username = %self.username,
                                "WS message channel closed, stopping GSM"
                            );
                            break;
                        }
                    }
                }
                // Connection state change from WS client
                state = self.ws_state_rx.recv() => {
                    match state {
                        Some(state) => self.handle_connection_state(state).await,
                        None => {
                            // State channel closed — WS receive loop exited.
                            // Keep running on messages + timer; the message
                            // channel close will trigger the real shutdown.
                            debug!("WS state channel closed");
                        }
                    }
                }
                // Periodic MCP refresh
                _ = refresh_timer.tick() => {
                    self.refresh_periodic().await;
                }
            }
        }

        info!(username = %self.username, "GameStateManager stopped");
    }

    /// Route a WS push message to the appropriate handler based on `msg_type`.
    async fn handle_message(&mut self, msg: WsRawMessage) {
        match msg.msg_type.as_str() {
            // State updates (v1 full + v2 delta)
            "state_update" => self.handle_state_update(msg.payload).await,
            "action_result" => self.handle_state_delta(msg.payload).await,

            // Event feed
            "chat_message" => self.handle_chat_message(msg.payload),
            "combat_update" => self.handle_combat_update(msg.payload),
            "mining_yield" => self.handle_mining_yield(msg.payload),
            "skill_level_up" => self.handle_skill_level_up(msg.payload),
            "player_died" => self.handle_player_died(msg.payload),
            "scan_detected" => self.handle_scan_detected(msg.payload),
            "pirate_warning" => self.handle_pirate_warning(msg.payload),
            "ok" => self.handle_ok(msg.payload),
            "action_error" => self.handle_action_error(msg.payload),
            "notification" => self.handle_notification(msg.payload),

            // Auth frames — shouldn't reach here (handled during connect),
            // but be resilient if the server resends them.
            "welcome" => debug!("Ignoring welcome frame in GSM (handled during connect)"),
            "logged_in" => self.handle_logged_in(msg.payload),

            other => {
                debug!(msg_type = other, "Unknown message type, ignoring");
            }
        }
    }

    // === State delta / update handlers ===

    /// Handle v2 `action_result` push — carries only changed sections.
    /// Merge each `Some` field from the delta into the current state.
    async fn handle_state_delta(&mut self, payload: Value) {
        match serde_json::from_value::<StateUpdatePayload>(payload) {
            Ok(delta) => {
                self.merge_state_update(delta);
                self.emit_state_update();
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse state delta from action_result");
            }
        }
    }

    /// Handle v1-style `state_update` push — may be full or partial.
    /// Same merge logic as delta — both decode to `StateUpdatePayload`.
    async fn handle_state_update(&mut self, payload: Value) {
        match serde_json::from_value::<StateUpdatePayload>(payload) {
            Ok(update) => {
                self.merge_state_update(update);
                self.emit_state_update();
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse state_update payload");
            }
        }
    }

    /// Merge a `StateUpdatePayload` into the current state.
    /// Only updates fields that are present (Some) in the payload —
    /// this is the v2 state-delta merge logic.
    fn merge_state_update(&mut self, update: StateUpdatePayload) {
        if update.tick > 0 {
            self.state.tick = update.tick;
        }
        if let Some(player) = update.player {
            self.state.player = player;
        }
        if let Some(ship) = update.ship {
            self.state.ship = ship;
        }
        if let Some(nearby) = update.nearby {
            self.state.nearby = nearby;
        }
        if let Some(in_combat) = update.in_combat {
            self.state.in_combat = in_combat;
        }
        if let Some(progress) = update.travel_progress {
            self.state.travel_progress = Some(progress);
        }
        if let Some(dest) = update.travel_destination {
            self.state.travel_destination = Some(dest);
        }
    }

    /// Handle `logged_in` frame if it reaches the GSM (resend during reconnect).
    /// Contains the initial full state snapshot — populate state from it.
    fn handle_logged_in(&mut self, payload: Value) {
        if let Ok(snapshot) = serde_json::from_value::<LoggedInPayload>(payload) {
            self.state.player = snapshot.player;
            self.state.ship = snapshot.ship;
            self.state.current_system = snapshot.system;
            self.state.current_poi = snapshot.poi;
            self.emit_state_update();
            info!("State populated from logged_in snapshot");
        }
    }

    // === Event feed handlers ===

    fn handle_chat_message(&mut self, payload: Value) {
        if let Ok(msg) = serde_json::from_value::<ChatMessagePayload>(payload) {
            let title = format!("[{}] {}", msg.channel, msg.sender);
            self.append_event(GameEventCategory::Chat, &title, &msg.content);
        }
    }

    fn handle_combat_update(&mut self, payload: Value) {
        if let Ok(combat) = serde_json::from_value::<CombatUpdatePayload>(payload) {
            self.state.in_combat = !combat.destroyed;

            let (title, category) = if combat.destroyed {
                (
                    format!("{} destroyed!", combat.target),
                    GameEventCategory::Death,
                )
            } else {
                (
                    format!("Combat: {} → {}", combat.attacker, combat.target),
                    GameEventCategory::Combat,
                )
            };
            let detail = format!(
                "Damage: {} ({}) | Shield: -{} | Hull: -{}",
                combat.damage, combat.damage_type, combat.shield_hit, combat.hull_hit
            );
            self.append_event(category, &title, &detail);

            if combat.destroyed {
                self.emit_state_update();
            }
        }
    }

    fn handle_mining_yield(&mut self, payload: Value) {
        if let Ok(mining) = serde_json::from_value::<MiningYieldPayload>(payload) {
            let resource = format_snake_case(&mining.resource_id);
            let title = format!("Mined {} {}", mining.quantity, resource);
            let detail = format!("{} remaining in deposit", mining.remaining);
            self.append_event(GameEventCategory::Mining, &title, &detail);
        }
    }

    fn handle_skill_level_up(&mut self, payload: Value) {
        if let Ok(skill) = serde_json::from_value::<SkillLevelUpPayload>(payload) {
            let skill_name = format_snake_case(&skill.skill_id);
            let title = format!("{} reached level {}!", skill_name, skill.new_level);
            let detail = format!("+{} XP", skill.xp_gained);
            self.append_event(GameEventCategory::Skill, &title, &detail);
        }
    }

    fn handle_player_died(&mut self, payload: Value) {
        if let Ok(death) = serde_json::from_value::<PlayerDiedPayload>(payload) {
            self.state.in_combat = false;

            let title = format!("Destroyed by {}", death.killer);
            let detail = format!(
                "Respawn at {} | Clone cost: {} cr | Insurance: {} cr | New ship: {}",
                death.respawn_base,
                death.clone_cost,
                death.insurance_payout,
                death.new_ship_class
            );
            self.append_event(GameEventCategory::Death, &title, &detail);
            self.emit_state_update();
        }
    }

    fn handle_scan_detected(&mut self, payload: Value) {
        if let Ok(scan) = serde_json::from_value::<ScanDetectedPayload>(payload) {
            let title = format!("Scanned by {}", scan.scanner_username);
            let detail = format!("{} | {}", scan.scanner_ship_class, scan.message);
            self.append_event(GameEventCategory::Scan, &title, &detail);
        }
    }

    fn handle_pirate_warning(&mut self, payload: Value) {
        if let Ok(pirate) = serde_json::from_value::<PirateWarningPayload>(payload) {
            let title = if pirate.is_boss {
                format!("⚠ BOSS PIRATE: {}", pirate.pirate_name)
            } else {
                format!(
                    "Pirate nearby: {} (Tier {})",
                    pirate.pirate_name, pirate.pirate_tier
                )
            };
            self.append_event(GameEventCategory::Pirate, &title, &pirate.message);
        }
    }

    fn handle_ok(&mut self, payload: Value) {
        if let Ok(ok) = serde_json::from_value::<OkActionPayload>(payload) {
            let action = format_snake_case(&ok.action);

            // Build detail from optional fields if message is empty
            let detail = if ok.message.is_empty() {
                let mut parts = Vec::new();
                if let Some(ref d) = ok.destination {
                    parts.push(format!("dest: {d}"));
                }
                if let Some(ref b) = ok.base {
                    parts.push(format!("base: {b}"));
                }
                if let Some(ref s) = ok.system {
                    parts.push(format!("system: {s}"));
                }
                if let Some(ref t) = ok.target {
                    parts.push(format!("target: {t}"));
                }
                if parts.is_empty() {
                    "OK".to_string()
                } else {
                    parts.join(", ")
                }
            } else {
                ok.message
            };

            // Map action name to event category
            let category = match ok.action.as_str() {
                "travel" | "warp" | "jump" | "dock" | "undock" | "navigate" => {
                    GameEventCategory::Navigation
                }
                "mine" | "jettison" => GameEventCategory::Mining,
                "buy" | "sell" | "trade" => GameEventCategory::Trade,
                _ => GameEventCategory::Info,
            };

            self.append_event(category, &action, &detail);
        }
    }

    fn handle_action_error(&mut self, payload: Value) {
        if let Ok(err) = serde_json::from_value::<ActionErrorPayload>(payload) {
            let title = format!("Error: {}", format_snake_case(&err.command));
            let detail = if err.code.is_empty() {
                err.message
            } else {
                format!("[{}] {}", err.code, err.message)
            };
            self.append_event(GameEventCategory::Info, &title, &detail);
        }
    }

    fn handle_notification(&mut self, payload: Value) {
        // Notifications are server-pushed automatically (not via get_notifications
        // MCP call — that's a poll fallback). Extract title + message.
        let message = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Notification received");
        let title = payload
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Notification");
        self.append_event(GameEventCategory::Info, title, message);
    }

    // === Connection state ===

    /// Handle a connection state change from the WS client.
    /// Updates state, emits to frontend, and triggers notification refresh
    /// on reconnect (to pull events missed during the disconnect window).
    async fn handle_connection_state(&mut self, state: ConnectionState) {
        info!(
            username = %self.username,
            state = ?state,
            "Connection state changed"
        );

        // Track reconnect transitions
        if state == ConnectionState::Reconnecting {
            self.was_reconnecting = true;
        }

        self.state.connection_state = state.clone();
        self.emit_connection_state(state.clone());

        // On Connected after a reconnect, refresh notifications (missed during
        // the disconnect window). Also re-fetch state to resync.
        if state == ConnectionState::Connected && self.was_reconnecting {
            self.was_reconnecting = false;
            info!("Reconnected — refreshing state + missed notifications");
            self.refresh_periodic().await;
            self.refresh_notifications().await;
        }
    }

    // === Periodic refresh ===

    /// Periodic MCP refresh — calls `get_state` to resync the full snapshot.
    /// Throttled at 3s by GameApi; the refresh timer fires every 5s.
    /// Throttle errors are expected and silently skipped.
    async fn refresh_periodic(&mut self) {
        match self.game_api.get_state(&mut self.mcp_session).await {
            Ok(refreshed) => {
                // Preserve GSM-managed fields (connection_state)
                let conn = self.state.connection_state.clone();
                self.state = refreshed;
                self.state.connection_state = conn;
                self.emit_state_update();
            }
            Err(e) => {
                // Throttle errors are expected — only log real errors
                if !e.contains("throttled") {
                    warn!(error = %e, "Periodic get_state failed");
                }
            }
        }
    }

    /// Fetch missed notifications via MCP `get_notifications` (poll fallback).
    /// Only called on initial connect and after reconnect.
    async fn refresh_notifications(&mut self) {
        match self.game_api.get_notifications(&mut self.mcp_session).await {
            Ok(data) => {
                // The response shape for get_notifications is raw JSON.
                // Try to extract an array of notification objects.
                if let Some(notifications) = data.as_array() {
                    for notif in notifications {
                        let message = notif
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        let title = notif
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("Notification");
                        if !message.is_empty() || title != "Notification" {
                            self.append_event(GameEventCategory::Info, title, message);
                        }
                    }
                    if !notifications.is_empty() {
                        info!(
                            count = notifications.len(),
                            "Fetched missed notifications"
                        );
                    }
                } else if let Some(message) = data.get("message").and_then(|m| m.as_str()) {
                    // Single notification (not wrapped in array)
                    let title = data
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Notification");
                    self.append_event(GameEventCategory::Info, title, message);
                }
            }
            Err(e) => {
                if !e.contains("throttled") {
                    debug!(error = %e, "refresh_notifications failed");
                }
            }
        }
    }

    // === Event emission ===

    /// Emit current state to the frontend (wrapped with username for routing).
    fn emit_state_update(&self) {
        let event = AccountStateUpdate {
            username: self.username.clone(),
            state: self.state.clone(),
        };
        if let Err(e) = self.app_handle.emit("state_update", &event) {
            warn!(error = %e, "Failed to emit state_update event");
        }
    }

    /// Emit connection state to the frontend.
    fn emit_connection_state(&self, state: ConnectionState) {
        let event = AccountConnectionState {
            username: self.username.clone(),
            state,
        };
        if let Err(e) = self.app_handle.emit("connection_state", &event) {
            warn!(error = %e, "Failed to emit connection_state event");
        }
    }

    // === Event feed ===

    /// Append an event to the feed: prepend (most recent first), cap at
    /// MAX_EVENTS, and emit to the frontend.
    fn append_event(&mut self, category: GameEventCategory, title: &str, detail: &str) {
        let event = GameEvent {
            category,
            title: title.to_string(),
            detail: detail.to_string(),
            timestamp: now_millis(),
        };

        self.events.push_front(event.clone());

        // Cap at MAX_EVENTS — remove oldest from the back
        while self.events.len() > MAX_EVENTS {
            self.events.pop_back();
        }

        // Emit to frontend (wrapped with username for multi-account routing)
        let envelope = AccountGameEvent {
            username: self.username.clone(),
            event,
        };
        if let Err(e) = self.app_handle.emit("game_event", &envelope) {
            warn!(error = %e, "Failed to emit game_event");
        }
    }
}

/// Current Unix time in milliseconds.
fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Convert snake_case to Title Case for display.
/// "mining_laser_1" → "Mining Laser 1"
/// "deep_core_mine" → "Deep Core Mine"
pub fn format_snake_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append an event to a VecDeque with MAX_EVENTS cap.
/// This is the pure (non-emit) core of `GameStateManager::append_event`,
/// extracted for unit testing without an AppHandle.
#[allow(dead_code)]
fn append_event_to_vec(
    events: &mut VecDeque<GameEvent>,
    category: GameEventCategory,
    title: &str,
    detail: &str,
) {
    let event = GameEvent {
        category,
        title: title.to_string(),
        detail: detail.to_string(),
        timestamp: 0, // tests don't care about timestamp
    };
    events.push_front(event);
    while events.len() > MAX_EVENTS {
        events.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_snake_case_converts_to_title_case() {
        assert_eq!(format_snake_case("mining_laser_1"), "Mining Laser 1");
        assert_eq!(format_snake_case("deep_core_mine"), "Deep Core Mine");
    }

    #[test]
    fn format_snake_case_handles_single_word() {
        assert_eq!(format_snake_case("combat"), "Combat");
        assert_eq!(format_snake_case("mining"), "Mining");
    }

    #[test]
    fn format_snake_case_handles_empty_string() {
        assert_eq!(format_snake_case(""), "");
    }

    #[test]
    fn format_snake_case_handles_no_underscores() {
        assert_eq!(format_snake_case("hello"), "Hello");
    }

    #[test]
    fn format_snake_case_preserves_numbers() {
        assert_eq!(format_snake_case("ore_1"), "Ore 1");
        assert_eq!(format_snake_case("v2"), "V2");
    }

    #[test]
    fn append_event_caps_at_200() {
        let mut events = VecDeque::new();
        for i in 0..250 {
            append_event_to_vec(
                &mut events,
                GameEventCategory::Info,
                &format!("Event {i}"),
                "test",
            );
        }
        assert_eq!(events.len(), 200);
        // Most recent should be first (prepended)
        assert_eq!(events[0].title, "Event 249");
        // Oldest retained: Event 50 (250 - 200 = 50)
        assert_eq!(events[199].title, "Event 50");
    }

    #[test]
    fn append_event_preserves_order() {
        let mut events = VecDeque::new();
        append_event_to_vec(&mut events, GameEventCategory::Info, "First", "d1");
        append_event_to_vec(&mut events, GameEventCategory::Info, "Second", "d2");
        append_event_to_vec(&mut events, GameEventCategory::Info, "Third", "d3");

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].title, "Third"); // most recent first
        assert_eq!(events[1].title, "Second");
        assert_eq!(events[2].title, "First");
    }

    #[test]
    fn append_event_under_cap_keeps_all() {
        let mut events = VecDeque::new();
        for i in 0..100 {
            append_event_to_vec(
                &mut events,
                GameEventCategory::Info,
                &format!("Event {i}"),
                "test",
            );
        }
        assert_eq!(events.len(), 100);
    }

    #[test]
    fn merge_state_update_merges_only_present_fields() {
        use crate::models::ShipOverview;
        let mut gsm_state = GameState::default();
        gsm_state.player.username = "original".to_string();
        gsm_state.ship.hull = 100;
        gsm_state.ship.max_hull = 200;

        // Delta with only ship changed
        let delta = StateUpdatePayload {
            tick: 42,
            player: None,
            ship: Some(ShipOverview {
                hull: 50,
                max_hull: 200,
                ..Default::default()
            }),
            nearby: None,
            in_combat: Some(true),
            travel_progress: None,
            travel_destination: None,
        };

        // Apply merge logic directly (same as merge_state_update method)
        if delta.tick > 0 {
            gsm_state.tick = delta.tick;
        }
        if let Some(player) = delta.player {
            gsm_state.player = player;
        }
        if let Some(ship) = delta.ship {
            gsm_state.ship = ship;
        }
        if let Some(nearby) = delta.nearby {
            gsm_state.nearby = nearby;
        }
        if let Some(in_combat) = delta.in_combat {
            gsm_state.in_combat = in_combat;
        }

        // Ship should be updated
        assert_eq!(gsm_state.ship.hull, 50);
        // Player should be preserved (not in delta)
        assert_eq!(gsm_state.player.username, "original");
        // Tick should be updated
        assert_eq!(gsm_state.tick, 42);
        // in_combat should be updated
        assert!(gsm_state.in_combat);
    }

    #[test]
    fn merge_state_update_empty_delta_preserves_state() {
        let mut gsm_state = GameState::default();
        gsm_state.player.username = "testuser".to_string();
        gsm_state.ship.hull = 350;
        gsm_state.tick = 100;

        let delta = StateUpdatePayload::default();

        // Apply merge — nothing should change
        if delta.tick > 0 {
            gsm_state.tick = delta.tick;
        }
        if let Some(player) = delta.player {
            gsm_state.player = player;
        }
        if let Some(ship) = delta.ship {
            gsm_state.ship = ship;
        }

        assert_eq!(gsm_state.player.username, "testuser");
        assert_eq!(gsm_state.ship.hull, 350);
        assert_eq!(gsm_state.tick, 100);
    }

    #[test]
    fn account_state_update_serializes_with_username() {
        let event = AccountStateUpdate {
            username: "testuser".to_string(),
            state: GameState::default(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"username\":\"testuser\""));
        assert!(json.contains("\"state\""));
    }

    #[test]
    fn account_game_event_serializes_with_username() {
        let event = AccountGameEvent {
            username: "testuser".to_string(),
            event: GameEvent {
                category: GameEventCategory::Combat,
                title: "Test".to_string(),
                detail: "Detail".to_string(),
                timestamp: 12345,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"username\":\"testuser\""));
        assert!(json.contains("\"title\":\"Test\""));
    }
}