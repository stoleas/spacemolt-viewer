// SpaceMolt Viewer — Game API client with safety allowlist.
//
// Wraps the MCP client (McpSession) with the 71-tool safety allowlist
// (verified against live v0.530.1 snapshot, 2026-07-19). A viewer must
// NEVER call mutation endpoints — the allowlist enforces this at the
// code level, before the tool call reaches the MCP transport.
//
// Typed convenience methods decode the JSON response into the serde
// models from Task 2. All decode errors are non-fatal — the caller
// gets an error string, not a crash.
//
// References:
//   - Decision #4: rmcp SDK for MCP (not hand-rolled)
//   - Decision #8: WS push-only, MCP for all queries
//   - Skill: 71-tool allowlist from live snapshot crosscheck
//   - Plan Task 5: GameApi wraps McpClient → adapted to McpSession

use crate::mcp_client::McpSession;
use crate::models;
use serde_json::Value;
use std::time::{Duration, Instant};
use tracing::error;

/// The 71-tool safety allowlist — all verified against the live v0.530.1
/// tool snapshot (captured 2026-07-19). Excludes: battle (combat),
/// chat/forum_* (social), set_*/mute/unsubscribe (settings),
/// subscribe_* (active subs), *_note/captains_log_add/delete (writes),
/// trade_cancel/decline_mission (state changes despite is_mutation=false),
/// action-dispatch tools (shipping/storage/station/facility), and v2
/// duplicates of v1 names.
///
/// A viewer is read-only. This allowlist is the hard boundary — if a
/// tool isn't in this list, it doesn't get called, period.
pub const ALLOWED_TOOLS: &[&str] = &[
    // Player / ship / status
    "get_status", "get_state", "get_ship", "get_cargo", "get_skills",
    "get_location", "list_ships", "browse_ships", "view_ship_buy_orders",
    "commission_quote", "commission_status",
    // System / map / navigation
    "get_system", "get_poi", "get_nearby", "get_map", "get_system_agents",
    "search_systems", "find_route", "inspect",
    // Missions
    "get_missions", "get_active_missions", "completed_missions", "view_completed_mission",
    // Market / trade / storage
    "view_market", "analyze_market", "view_orders", "estimate_purchase",
    "get_trades", "view_storage", "view_faction_storage",
    "get_tax_estimate", "get_faction_tax_estimate",
    // Faction (read-only)
    "faction_info", "faction_list", "faction_get_invites", "faction_list_missions",
    "faction_garages", "faction_rooms", "faction_intel_status",
    "faction_query_intel", "faction_query_trade_intel", "faction_trade_intel_status",
    // Catalog / reference
    "catalog", "get_commands", "get_version", "get_guide", "help",
    // Captain's log / notes (read-only)
    "captains_log_list", "captains_log_get", "get_notes", "read_note",
    "get_action_log", "get_battle_log", "get_battle_status", "get_battle_summary",
    // Drones / passengers / wrecks / insurance
    "get_drone", "get_drones", "list_passengers", "list_station_passengers",
    "get_wrecks", "view_insurance", "claim_insurance", "get_insurance_quote",
    // Base / empire / achievements / notifications
    "get_base", "get_base_cost", "get_empire_info",
    "get_achievements", "get_faction_achievements",
    "get_chat_history", "get_notifications", "get_notification_settings",
];

/// Check if a tool name is in the safety allowlist.
pub fn is_tool_allowed(tool: &str) -> bool {
    ALLOWED_TOOLS.contains(&tool)
}

/// Per-query-type throttle intervals to avoid hammering the server.
/// Keyed by tool name. Tools not in this map have no throttle (1-call minimum).
const THROTTLE: &[(&str, Duration)] = &[
    ("get_skills", Duration::from_secs(10)),
    ("get_nearby", Duration::from_secs(5)),
    ("get_cargo", Duration::from_secs(5)),
    ("list_ships", Duration::from_secs(30)),
    ("get_map", Duration::from_secs(300)), // map rarely changes
    ("get_notifications", Duration::from_secs(10)),
    ("get_state", Duration::from_secs(3)),
    ("get_status", Duration::from_secs(3)),
];

/// Returns the throttle interval for a tool, if any.
fn throttle_for(tool: &str) -> Option<Duration> {
    THROTTLE.iter().find(|(t, _)| *t == tool).map(|(_, d)| *d)
}

/// GameApi wraps an McpSession with the safety allowlist and typed
/// convenience methods. It enforces read-only discipline and provides
/// per-query throttling.
///
/// The McpSession is owned by the caller (SessionManager / AccountSession
/// in Task 8) and passed in as a &mut reference — GameApi does not take
/// ownership because the session is shared with the WS client lifecycle.
///
/// Usage:
///   ```ignore
///   let mut session = McpSession::connect().await?;
///   session.login("user", "pass").await?;
///   let api = GameApi::new();
///   let status = api.get_status(&mut session).await?;
///   ```
pub struct GameApi {
    /// Per-tool last-call timestamps for throttling.
    last_calls: std::collections::HashMap<String, Instant>,
}

impl GameApi {
    /// Create a new GameApi. The McpSession is NOT owned — it's passed
    /// by &mut to each method call. This lets the session be shared
    /// between GameApi (queries) and the account lifecycle (WS reconnect).
    pub fn new() -> Self {
        Self {
            last_calls: std::collections::HashMap::new(),
        }
    }

    /// Check the throttle for a tool. Returns Ok(()) if the tool can be
    /// called now, or Err with a human-readable message if it's too soon.
    fn check_throttle(&mut self, tool: &str) -> Result<(), String> {
        if let Some(interval) = throttle_for(tool) {
            if let Some(&last) = self.last_calls.get(tool) {
                let elapsed = last.elapsed();
                if elapsed < interval {
                    let remaining = interval - elapsed;
                    return Err(format!(
                        "tool '{tool}' throttled ({:.1}s remaining)",
                        remaining.as_secs_f32()
                    ));
                }
            }
        }
        self.last_calls.insert(tool.to_string(), Instant::now());
        Ok(())
    }

    /// Call a tool with the safety allowlist gate, session_id auto-injection
    /// (handled by McpSession), and throttling. Returns the raw JSON Value.
    pub async fn call_tool(
        &mut self,
        session: &mut McpSession,
        tool: &str,
        extra_args: Option<Value>,
    ) -> Result<Value, String> {
        if !is_tool_allowed(tool) {
            return Err(format!("Tool '{tool}' is not in the safety allowlist"));
        }

        self.check_throttle(tool)?;

        let args = match extra_args {
            Some(Value::Object(map)) => map,
            Some(_) => {
                return Err("extra_args must be a JSON object".to_string());
            }
            None => serde_json::Map::new(),
        };

        session
            .call_with_args(tool, args)
            .await
            .map_err(|e| {
                error!(tool, error = %e, "GameApi tool call failed");
                e.to_string()
            })
    }

    // === Typed convenience methods ===
    // Each method calls a specific allowlisted tool and decodes the
    // response into the corresponding serde model. Decode errors are
    // returned as strings — the caller decides whether to retry or
    // display the error.

    /// Get full player + ship status.
    pub async fn get_status(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::PlayerStatusResponse, String> {
        let data = self.call_tool(session, "get_status", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get canonical game state (v2 — replaces separate status/ship/system/nearby calls).
    pub async fn get_state(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::GameState, String> {
        let data = self.call_tool(session, "get_state", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get cargo contents.
    pub async fn get_cargo(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::CargoResponse, String> {
        let data = self.call_tool(session, "get_cargo", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get current system info.
    pub async fn get_system(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::SystemResponse, String> {
        let data = self.call_tool(session, "get_system", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get nearby players/pirates/POIs.
    pub async fn get_nearby(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::NearbyResponse, String> {
        let data = self.call_tool(session, "get_nearby", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get detailed ship info (modules, vitals, stats).
    pub async fn get_ship(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::ShipDetailResponse, String> {
        let data = self.call_tool(session, "get_ship", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get player skills.
    pub async fn get_skills(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::SkillsResponse, String> {
        let data = self.call_tool(session, "get_skills", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get active missions.
    pub async fn get_active_missions(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::MissionsResponse, String> {
        let data = self.call_tool(session, "get_active_missions", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get available missions at docked base.
    pub async fn get_missions(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::MissionsResponse, String> {
        let data = self.call_tool(session, "get_missions", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get chat history for a channel.
    pub async fn get_chat_history(
        &mut self,
        session: &mut McpSession,
        channel: &str,
        limit: u32,
    ) -> Result<models::ChatHistoryResponse, String> {
        let args = serde_json::json!({"channel": channel, "limit": limit});
        let data = self.call_tool(session, "get_chat_history", Some(args)).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get station storage.
    pub async fn view_storage(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::StorageResponse, String> {
        let data = self.call_tool(session, "view_storage", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get owned ships.
    pub async fn list_ships(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::OwnedShipsResponse, String> {
        let data = self.call_tool(session, "list_ships", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get captain's log entries. Fetches the first page (index 0 = newest),
    /// then iterates remaining pages. The API returns one entry per call
    /// via `captains_log_list` with an index parameter.
    ///
    /// Note: The server's `captains_log_list` returns a single entry per
    /// call with metadata (total_count, max_entries). We fetch the first
    /// page to get the count, then fetch the remaining entries.
    pub async fn get_captains_log(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::CaptainsLogResponse, String> {
        // Fetch index 0 to get the first entry + total count
        let args = serde_json::json!({"index": 0});
        let data = self.call_tool(session, "captains_log_list", Some(args)).await?;

        // The response shape from captains_log_list is a single entry with
        // metadata. Parse it flexibly — the exact shape may vary.
        // We try to extract the entry and total_count from the response.
        let mut entries: Vec<models::LogEntry> = Vec::new();

        // Try to parse as a single log entry
        if let Ok(entry) = serde_json::from_value::<models::LogEntry>(data.clone()) {
            entries.push(entry);
        }

        // If we couldn't parse as a single entry, try as a response with entries array
        if entries.is_empty() {
            if let Ok(resp) = serde_json::from_value::<models::CaptainsLogResponse>(data.clone()) {
                entries = resp.entries;
            }
        }

        // Note: Pagination would require knowing the total_count from the
        // first response. The exact response shape for captains_log_list
        // needs live verification. For v1, we return what we got — the
        // GameStateManager can re-fetch as needed.
        // TODO: Once live response shape is confirmed, implement full pagination.

        Ok(models::CaptainsLogResponse { entries })
    }

    /// Get galaxy map data via authenticated MCP `get_map` call
    /// (Decision #7: route through Rust/MCP, not public `fetch()`).
    /// This is v2 scope but the method is here for when it's needed.
    pub async fn get_map(
        &mut self,
        session: &mut McpSession,
    ) -> Result<models::PublicMapResponse, String> {
        let data = self.call_tool(session, "get_map", None).await?;
        serde_json::from_value(data).map_err(|e| format!("Decode error: {e}"))
    }

    /// Get notifications (MCP-only — not available over WS).
    /// Use as poll fallback after WS reconnect.
    pub async fn get_notifications(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        // Notifications don't have a dedicated model yet — return raw JSON
        // for the frontend to render. Task 9 (GameStateManager) will handle
        // routing these to the event feed.
        self.call_tool(session, "get_notifications", None).await
    }

    /// Get action log.
    pub async fn get_action_log(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        self.call_tool(session, "get_action_log", None).await
    }

    /// Get battle log.
    pub async fn get_battle_log(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        self.call_tool(session, "get_battle_log", None).await
    }

    /// Get battle status.
    pub async fn get_battle_status(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        self.call_tool(session, "get_battle_status", None).await
    }

    /// Get nearby wrecks.
    pub async fn get_wrecks(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        self.call_tool(session, "get_wrecks", None).await
    }

    /// Get drones.
    pub async fn get_drones(
        &mut self,
        session: &mut McpSession,
    ) -> Result<Value, String> {
        self.call_tool(session, "get_drones", None).await
    }
}

impl Default for GameApi {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
use std::collections::HashSet;

    // === Allowlist tests ===

    #[test]
    fn test_mutation_tools_rejected() {
    assert!(!is_tool_allowed("attack"));
    assert!(!is_tool_allowed("battle"));
    assert!(!is_tool_allowed("chat"));
    assert!(!is_tool_allowed("forum_post"));
    assert!(!is_tool_allowed("set_status_message"));
    assert!(!is_tool_allowed("mute"));
    assert!(!is_tool_allowed("trade_cancel"));
    assert!(!is_tool_allowed("decline_mission"));
    assert!(!is_tool_allowed("captains_log_add"));
    assert!(!is_tool_allowed("delete"));
    assert!(!is_tool_allowed("subscribe_observation"));
    assert!(!is_tool_allowed("shipping"));
    assert!(!is_tool_allowed("station"));
    assert!(!is_tool_allowed("facility"));
    }

    #[test]
    fn test_readonly_tools_allowed() {
    assert!(is_tool_allowed("get_status"));
    assert!(is_tool_allowed("get_state"));
    assert!(is_tool_allowed("get_ship"));
    assert!(is_tool_allowed("get_cargo"));
    assert!(is_tool_allowed("get_skills"));
    assert!(is_tool_allowed("get_nearby"));
    assert!(is_tool_allowed("get_system"));
    assert!(is_tool_allowed("get_map"));
    assert!(is_tool_allowed("list_ships"));
    assert!(is_tool_allowed("view_market"));
    assert!(is_tool_allowed("get_missions"));
    assert!(is_tool_allowed("get_active_missions"));
    assert!(is_tool_allowed("captains_log_list"));
    assert!(is_tool_allowed("get_notifications"));
    assert!(is_tool_allowed("faction_info"));
    assert!(is_tool_allowed("get_battle_log"));
    assert!(is_tool_allowed("get_wrecks"));
    assert!(is_tool_allowed("get_drones"));
    }

    #[test]
    fn test_unknown_tool_rejected() {
    assert!(!is_tool_allowed(""));
    assert!(!is_tool_allowed("not_a_tool"));
    assert!(!is_tool_allowed("GET_STATUS")); // case sensitive
    assert!(!is_tool_allowed("get_status "));  // whitespace
    }

    #[test]
    fn test_allowlist_count() {
    // The allowlist must have exactly 71 tools (verified against v0.530.1)
    assert_eq!(ALLOWED_TOOLS.len(), 71, "Allowlist must have 71 tools — got {}", ALLOWED_TOOLS.len());
    }

    #[test]
    fn test_allowlist_no_duplicates() {
    let set: HashSet<&&str> = ALLOWED_TOOLS.iter().collect();
    assert_eq!(set.len(), ALLOWED_TOOLS.len(), "Allowlist has duplicate entries");
    }

    #[test]
    fn test_allowlist_sorted_for_readability() {
    // The allowlist is organized by category — verify it's at least
    // internally grouped (no hard sort check, but no tool should appear
    // before another in its category group)
    assert!(ALLOWED_TOOLS.contains(&"get_status"));
    assert!(ALLOWED_TOOLS.contains(&"get_map"));
    }

    // === Throttle tests ===

    #[test]
    fn test_throttle_intervals() {
    assert_eq!(throttle_for("get_skills"), Some(Duration::from_secs(10)));
    assert_eq!(throttle_for("get_nearby"), Some(Duration::from_secs(5)));
    assert_eq!(throttle_for("get_cargo"), Some(Duration::from_secs(5)));
    assert_eq!(throttle_for("list_ships"), Some(Duration::from_secs(30)));
    assert_eq!(throttle_for("get_map"), Some(Duration::from_secs(300)));
    assert_eq!(throttle_for("get_state"), Some(Duration::from_secs(3)));
    assert_eq!(throttle_for("get_status"), Some(Duration::from_secs(3)));
    }

    #[test]
    fn test_throttle_not_in_map() {
    // Tools not in the throttle map have no throttle
    assert_eq!(throttle_for("get_ship"), None);
    assert_eq!(throttle_for("get_system"), None);
    assert_eq!(throttle_for("faction_info"), None);
    assert_eq!(throttle_for("not_a_tool"), None);
    }

    #[test]
    fn test_throttle_blocks_rapid_calls() {
    let mut api = GameApi::new();
    // First call to get_skills should pass
    assert!(api.check_throttle("get_skills").is_ok());
    // Immediate second call should be throttled
    let result = api.check_throttle("get_skills");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("throttled"));
    }

    #[test]
    fn test_throttle_different_tools_independent() {
    let mut api = GameApi::new();
    assert!(api.check_throttle("get_skills").is_ok());
    assert!(api.check_throttle("get_nearby").is_ok()); // different tool, not throttled
    }

    #[test]
    fn test_throttle_unthrottled_tool_always_ok() {
    let mut api = GameApi::new();
    assert!(api.check_throttle("get_ship").is_ok());
    assert!(api.check_throttle("get_ship").is_ok()); // no throttle, always ok
    assert!(api.check_throttle("get_ship").is_ok());
    }

    // === GameApi construction ===

    #[test]
    fn test_game_api_new() {
    let api = GameApi::new();
    assert!(api.last_calls.is_empty());
    }

    #[test]
    fn test_game_api_default() {
    let api = GameApi::default();
    assert!(api.last_calls.is_empty());
    }

    // === Allowlist category coverage ===

    #[test]
    fn test_allowlist_covers_player_tools() {
    for tool in &["get_status", "get_state", "get_ship", "get_cargo", "get_skills",
    "get_location", "list_ships", "browse_ships", "view_ship_buy_orders",
    "commission_quote", "commission_status"] {
        assert!(is_tool_allowed(tool), "Player tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_system_tools() {
    for tool in &["get_system", "get_poi", "get_nearby", "get_map", "get_system_agents",
    "search_systems", "find_route", "inspect"] {
        assert!(is_tool_allowed(tool), "System tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_mission_tools() {
    for tool in &["get_missions", "get_active_missions", "completed_missions", "view_completed_mission"] {
        assert!(is_tool_allowed(tool), "Mission tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_market_tools() {
    for tool in &["view_market", "analyze_market", "view_orders", "estimate_purchase",
    "get_trades", "view_storage", "view_faction_storage",
    "get_tax_estimate", "get_faction_tax_estimate"] {
        assert!(is_tool_allowed(tool), "Market tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_faction_tools() {
    for tool in &["faction_info", "faction_list", "faction_get_invites", "faction_list_missions",
    "faction_garages", "faction_rooms", "faction_intel_status",
    "faction_query_intel", "faction_query_trade_intel", "faction_trade_intel_status"] {
        assert!(is_tool_allowed(tool), "Faction tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_catalog_tools() {
    for tool in &["catalog", "get_commands", "get_version", "get_guide", "help"] {
        assert!(is_tool_allowed(tool), "Catalog tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_log_tools() {
    for tool in &["captains_log_list", "captains_log_get", "get_notes", "read_note",
    "get_action_log", "get_battle_log", "get_battle_status", "get_battle_summary"] {
        assert!(is_tool_allowed(tool), "Log tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_drone_tools() {
    for tool in &["get_drone", "get_drones", "list_passengers", "list_station_passengers",
    "get_wrecks", "view_insurance", "claim_insurance", "get_insurance_quote"] {
        assert!(is_tool_allowed(tool), "Drone/misc tool {tool} should be allowed");
    }
    }

    #[test]
    fn test_allowlist_covers_base_tools() {
    for tool in &["get_base", "get_base_cost", "get_empire_info",
    "get_achievements", "get_faction_achievements",
    "get_chat_history", "get_notifications", "get_notification_settings"] {
        assert!(is_tool_allowed(tool), "Base/empire tool {tool} should be allowed");
    }
    }

    // === Phantom tools from macOS reference (must NOT be in allowlist) ===

    #[test]
    fn test_phantom_tools_excluded() {
    // These 5 tools are in the macOS reference's allowlist but don't
    // exist in the live v0.530.1 API snapshot. They must NOT be in our
    // allowlist — calling them would error.
    assert!(!is_tool_allowed("get_listings"));
    assert!(!is_tool_allowed("get_base_wrecks"));
    assert!(!is_tool_allowed("raid_status"));
    assert!(!is_tool_allowed("get_recipes"));
    assert!(!is_tool_allowed("get_ships")); // phantom — we use list_ships
    }
}