// SpaceMolt Viewer — serde data models.
//
// All models use `#[serde(default)]` on every field so missing/null fields
// don't cause parse failures (Decision #7: resilient decoding). Field names
// match the server JSON directly (snake_case) — no rename needed.
//
// Sources:
//   - Plan Task 2 struct list
//   - references/spacemolt-api-reference.md (live API response shapes)
//   - references/live-snapshot-crosscheck.md (v0.530.1 verified fields)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// === Player / status (get_status + state_update player section) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PlayerStatusResponse {
    #[serde(default)]
    pub player: Player,
    #[serde(default)]
    pub ship: ShipOverview,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Player {
    #[serde(default)] pub id: String,
    #[serde(default)] pub username: String,
    #[serde(default)] pub empire: String,
    #[serde(default)] pub credits: i64,
    #[serde(default)] pub created_at: String,
    #[serde(default)] pub last_login_at: String,
    #[serde(default)] pub last_active_at: String,
    #[serde(default)] pub status_message: String,
    #[serde(default)] pub clan_tag: String,
    #[serde(default)] pub primary_color: String,
    #[serde(default)] pub secondary_color: String,
    #[serde(default)] pub anonymous: bool,
    #[serde(default)] pub is_cloaked: bool,
    #[serde(default)] pub current_ship_id: String,
    #[serde(default)] pub current_system: String,
    #[serde(default)] pub current_poi: String,
    #[serde(default)] pub home_base: String,
    #[serde(default)] pub docked_at_base: String,
    #[serde(default)] pub skills: HashMap<String, i64>,
    #[serde(default)] pub skill_xp: HashMap<String, i64>,
    #[serde(default)] pub experience: i64,
    #[serde(default)] pub stats: PlayerStats,
    #[serde(default)] pub discovered_systems: HashMap<String, bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PlayerStats {
    #[serde(default)] pub credits_earned: i64,
    #[serde(default)] pub credits_spent: i64,
    #[serde(default)] pub ships_destroyed: i64,
    #[serde(default)] pub ships_lost: i64,
    #[serde(default)] pub pirates_destroyed: i64,
    #[serde(default)] pub ore_mined: i64,
    #[serde(default)] pub items_crafted: i64,
    #[serde(default)] pub trades_completed: i64,
    #[serde(default)] pub systems_explored: i64,
}

// === Ship overview (get_status.ship + state_update.ship) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipOverview {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub class_id: String,
    #[serde(default)] pub hull: i64,
    #[serde(default)] pub max_hull: i64,
    #[serde(default)] pub shield: i64,
    #[serde(default)] pub max_shield: i64,
    #[serde(default)] pub armor: i64,
    #[serde(default)] pub fuel: i64,
    #[serde(default)] pub max_fuel: i64,
    #[serde(default)] pub speed: i64,
    #[serde(default)] pub cargo_used: i64,
    #[serde(default)] pub cargo_capacity: i64,
    #[serde(default)] pub cpu_used: i64,
    #[serde(default)] pub cpu_max: i64,
    #[serde(default)] pub power_used: i64,
    #[serde(default)] pub power_max: i64,
}

impl ShipOverview {
    pub fn hull_percent(&self) -> f64 { pct(self.hull, self.max_hull) }
    pub fn shield_percent(&self) -> f64 { pct(self.shield, self.max_shield) }
    pub fn fuel_percent(&self) -> f64 { pct(self.fuel, self.max_fuel) }
    pub fn cargo_percent(&self) -> f64 { pct(self.cargo_used, self.cargo_capacity) }
}

fn pct(val: i64, max: i64) -> f64 {
    if max == 0 { 0.0 } else { val as f64 / max as f64 * 100.0 }
}

// === Cargo (get_cargo) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CargoResponse {
    #[serde(default)] pub available: i64,
    #[serde(default)] pub capacity: i64,
    #[serde(default)] pub used: i64,
    #[serde(default)] pub cargo: Vec<CargoItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CargoItem {
    #[serde(default)] pub item_id: String,
    #[serde(default)] pub quantity: i64,
    #[serde(default)] pub size: i64,
}

// === System (get_system) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemResponse {
    #[serde(default)] pub system: StarSystem,
    #[serde(default)] pub pois: Vec<PointOfInterest>,
    #[serde(default)] pub security_status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StarSystem {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub police_level: i64,
    #[serde(default)] pub connections: Vec<String>,
    #[serde(default)] pub pois: Vec<String>,
    #[serde(default)] pub position: SystemPosition,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemPosition {
    #[serde(default)] pub x: i64,
    #[serde(default)] pub y: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PointOfInterest {
    #[serde(default)] pub id: String,
    #[serde(default)] pub system_id: String,
    #[serde(default)] pub r#type: String, // `type` is a Rust keyword → r#type
    #[serde(default)] pub name: String,
    #[serde(default)] pub position: SystemPosition,
}

// === Nearby (get_nearby) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NearbyResponse {
    #[serde(default)] pub count: i64,
    #[serde(default)] pub nearby: Vec<NearbyPlayer>,
    #[serde(default)] pub pirate_count: i64,
    #[serde(default)] pub pirates: Vec<Pirate>,
    #[serde(default)] pub poi_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NearbyPlayer {
    #[serde(default)] pub player_id: String,
    #[serde(default)] pub username: String,
    #[serde(default)] pub ship_class: String,
    #[serde(default)] pub clan_tag: String,
    #[serde(default)] pub anonymous: bool,
    #[serde(default)] pub in_combat: bool,
    #[serde(default)] pub status_message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Pirate {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub ship_class: String,
    #[serde(default)] pub hull_percent: f64,
}

// === Ship detail (get_ship) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipDetailResponse {
    #[serde(default)] pub cargo_max: i64,
    #[serde(default)] pub cargo_used: i64,
    #[serde(default)] pub class: ShipClass,
    #[serde(default)] pub modules: Vec<ShipModule>,
    #[serde(default)] pub ship: ShipVitals,
    #[serde(default)] pub stats: ShipStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipClass {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub class: String, // "Mining", "Combat", etc.
    #[serde(default)] pub price: i64,
    #[serde(default)] pub base_hull: i64,
    #[serde(default)] pub base_shield: i64,
    #[serde(default)] pub base_armor: i64,
    #[serde(default)] pub base_speed: i64,
    #[serde(default)] pub base_fuel: i64,
    #[serde(default)] pub cargo_capacity: i64,
    #[serde(default)] pub cpu_capacity: i64,
    #[serde(default)] pub power_capacity: i64,
    #[serde(default)] pub weapon_slots: i64,
    #[serde(default)] pub defense_slots: i64,
    #[serde(default)] pub utility_slots: i64,
    #[serde(default)] pub default_modules: Vec<ShipModule>,
    #[serde(default)] pub required_skills: HashMap<String, i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipModule {
    #[serde(default)] pub id: String,
    #[serde(default)] pub type_id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub r#type: String, // mining, weapon, defense, utility
    #[serde(default)] pub cpu_usage: i64,
    #[serde(default)] pub power_usage: i64,
    #[serde(default)] pub mining_power: i64,
    #[serde(default)] pub mining_range: i64,
    #[serde(default)] pub quality: i64,
    #[serde(default)] pub quality_grade: String,
    #[serde(default)] pub wear: i64,
    #[serde(default)] pub wear_status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipVitals {
    #[serde(default)] pub armor: i64,
    #[serde(default)] pub fuel: i64,
    #[serde(default)] pub hull: i64,
    #[serde(default)] pub max_fuel: i64,
    #[serde(default)] pub max_hull: i64,
    #[serde(default)] pub max_shield: i64,
    #[serde(default)] pub shield: i64,
    #[serde(default)] pub speed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShipStats {
    #[serde(default)] pub cpu_max: i64,
    #[serde(default)] pub cpu_used: i64,
    #[serde(default)] pub power_max: i64,
    #[serde(default)] pub power_used: i64,
}

// === Skills (get_skills) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SkillsResponse {
    #[serde(default)] pub player_skill_count: i64,
    #[serde(default)] pub player_skills: Vec<Skill>,
    #[serde(default)] pub total_skill_count: i64,
    #[serde(default)] pub all_skills: Vec<Skill>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Skill {
    #[serde(default)] pub skill_id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub category: String,
    #[serde(default)] pub level: i64,
    #[serde(default)] pub current_xp: i64,
    #[serde(default)] pub next_level_xp: i64,
    #[serde(default)] pub max_level: i64,
}

// === Missions (get_active_missions + get_missions) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MissionsResponse {
    #[serde(default)] pub missions: Option<Vec<Mission>>, // null when empty
    #[serde(default)] pub total_count: i64,
    #[serde(default)] pub max_missions: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Mission {
    #[serde(default)] pub id: String,
    #[serde(default)] pub r#type: String,
    #[serde(default)] pub title: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub difficulty: i64,
    #[serde(default)] pub objectives: Vec<MissionObjective>,
    #[serde(default)] pub rewards: MissionRewards,
    #[serde(default)] pub expires_at: String,
    #[serde(default)] pub ticks_remaining: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MissionObjective {
    #[serde(default)] pub description: String,
    #[serde(default)] pub current: i64,
    #[serde(default)] pub required: i64,
    #[serde(default)] pub completed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MissionRewards {
    #[serde(default)] pub credits: i64,
    #[serde(default)] pub skill_xp: HashMap<String, i64>,
}

// === Owned ships (list_ships) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OwnedShipsResponse {
    #[serde(default)] pub active_ship_class: String,
    #[serde(default)] pub active_ship_id: String,
    #[serde(default)] pub count: i64,
    #[serde(default)] pub ships: Vec<OwnedShip>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OwnedShip {
    #[serde(default)] pub ship_id: String,
    #[serde(default)] pub class_id: String,
    #[serde(default)] pub class_name: String,
    #[serde(default)] pub is_active: bool,
    #[serde(default)] pub location: String,
    #[serde(default)] pub hull: String, // "346/350" — string, not numeric
    #[serde(default)] pub fuel: String, // "130/200" — string, not numeric
    #[serde(default)] pub cargo_used: i64,
    #[serde(default)] pub modules: i64,
}

// === Storage (view_storage) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StorageResponse {
    #[serde(default)] pub credits: i64,
    #[serde(default)] pub items: Vec<StorageItem>,
    #[serde(default)] pub station_name: String,
    #[serde(default)] pub station_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StorageItem {
    #[serde(default)] pub item_id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub quantity: i64,
}

// === Market (view_market) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MarketResponse {
    #[serde(default)] pub buy_orders: Vec<MarketOrder>,
    #[serde(default)] pub sell_orders: Vec<MarketOrder>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MarketOrder {
    #[serde(default)] pub item_id: String,
    #[serde(default)] pub price: i64,
    #[serde(default)] pub total_quantity: i64,
}

// === Chat (get_chat_history + chat_message push event) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatHistoryResponse {
    #[serde(default)] pub messages: Vec<ChatMessage>,
    #[serde(default)] pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessage {
    #[serde(default)] pub id: String,
    #[serde(default)] pub channel: String, // system|local|faction|private
    #[serde(default)] pub sender_id: String,
    #[serde(default)] pub sender_name: String,
    #[serde(default)] pub content: String,
    #[serde(default)] pub timestamp: String, // RFC3339
}

// === Captain's log (captains_log_list) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CaptainsLogResponse {
    #[serde(default)] pub entries: Vec<LogEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LogEntry {
    #[serde(default)] pub index: i64, // 0 = newest
    #[serde(default)] pub entry: String,
    #[serde(default)] pub created_at: String,
}

// === Galaxy map (get_map) — v2 scope, but define the model now ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PublicMapResponse {
    #[serde(default)] pub systems: Vec<MapSystem>,
    #[serde(default)] pub total: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MapSystem {
    #[serde(default)] pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub position: SystemPosition,
    #[serde(default)] pub connections: Vec<String>,
    #[serde(default)] pub police_level: i64,
    #[serde(default)] pub security_status: String,
}

// === Game state (composite — built by GameStateManager from push events) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GameState {
    #[serde(default)] pub tick: i64,
    #[serde(default)] pub player: Player,
    #[serde(default)] pub ship: ShipOverview,
    #[serde(default)] pub nearby: Vec<NearbyPlayer>,
    #[serde(default)] pub pirates: Vec<Pirate>,
    #[serde(default)] pub in_combat: bool,
    #[serde(default)] pub travel_progress: Option<f64>,
    #[serde(default)] pub travel_destination: Option<String>,
    #[serde(default)] pub current_system: StarSystem,
    #[serde(default)] pub current_poi: PointOfInterest,
    #[serde(default)] pub cargo: CargoResponse,
    #[serde(default)] pub skills: SkillsResponse,
    #[serde(default)] pub missions: MissionsResponse,
    #[serde(default)] pub connection_state: ConnectionState,
}

// === Connection state (emitted to frontend) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    SessionExpired,
    SessionReplaced, // WS close code 4001 — do NOT reconnect
    RateLimited { retry_after: u64 }, // WS close code 4003
    Error,
    Offline,
}

// === Game event feed (emitted to frontend) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GameEvent {
    #[serde(default)] pub category: GameEventCategory,
    #[serde(default)] pub title: String,
    #[serde(default)] pub detail: String,
    #[serde(default)] pub timestamp: i64, // unix millis
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GameEventCategory {
    #[default]
    Info,
    Combat,
    Mining,
    Navigation,
    Trade,
    Skill,
    Pirate,
    Scan,
    Base,
    System,
    Broadcast,
    Chat,
    Death,
}

// === WebSocket push event payloads (v2 framing — Decision #1) ===

/// `welcome` frame — received on WS connect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WelcomePayload {
    #[serde(default)] pub version: String,
    #[serde(default)] pub tick_rate: i64,
    #[serde(default)] pub current_tick: i64,
    #[serde(default)] pub server_time: String,
    #[serde(default)] pub motd: String,
}

/// `logged_in` frame — received after login.
/// Contains the initial full state snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoggedInPayload {
    #[serde(default)] pub player: Player,
    #[serde(default)] pub ship: ShipOverview,
    #[serde(default)] pub system: StarSystem,
    #[serde(default)] pub poi: PointOfInterest,
    #[serde(default)] pub captains_log: CaptainsLogResponse,
    #[serde(default)] pub unread_chat: i64,
}

/// `state_update` frame (v1) / `action_result` with state delta (v2).
/// v2 sends only changed sections — `GameStateManager` merges these.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StateUpdatePayload {
    #[serde(default)] pub tick: i64,
    #[serde(default)] pub player: Option<Player>,
    #[serde(default)] pub ship: Option<ShipOverview>,
    #[serde(default)] pub nearby: Option<Vec<NearbyPlayer>>,
    #[serde(default)] pub in_combat: Option<bool>,
    #[serde(default)] pub travel_progress: Option<f64>,
    #[serde(default)] pub travel_destination: Option<String>,
}

/// `chat_message` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessagePayload {
    #[serde(default)] pub channel: String,
    #[serde(default)] pub sender: String,
    #[serde(default)] pub content: String,
    #[serde(default)] pub timestamp: String,
}

/// `combat_update` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CombatUpdatePayload {
    #[serde(default)] pub tick: i64,
    #[serde(default)] pub attacker: String,
    #[serde(default)] pub target: String,
    #[serde(default)] pub damage: i64,
    #[serde(default)] pub damage_type: String,
    #[serde(default)] pub shield_hit: i64,
    #[serde(default)] pub hull_hit: i64,
    #[serde(default)] pub destroyed: bool,
}

/// `mining_yield` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MiningYieldPayload {
    #[serde(default)] pub resource_id: String,
    #[serde(default)] pub quantity: i64,
    #[serde(default)] pub remaining: i64,
}

/// `skill_level_up` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SkillLevelUpPayload {
    #[serde(default)] pub skill_id: String,
    #[serde(default)] pub new_level: i64,
    #[serde(default)] pub xp_gained: i64,
}

/// `player_died` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PlayerDiedPayload {
    #[serde(default)] pub killer: String,
    #[serde(default)] pub respawn_base: String,
    #[serde(default)] pub clone_cost: i64,
    #[serde(default)] pub insurance_payout: i64,
    #[serde(default)] pub new_ship_class: String,
}

/// `scan_detected` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ScanDetectedPayload {
    #[serde(default)] pub scanner_username: String,
    #[serde(default)] pub scanner_ship_class: String,
    #[serde(default)] pub message: String,
}

/// `pirate_warning` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PirateWarningPayload {
    #[serde(default)] pub pirate_name: String,
    #[serde(default)] pub pirate_tier: i64,
    #[serde(default)] pub is_boss: bool,
    #[serde(default)] pub message: String,
}

/// `ok` / `action_result` push event — bot action confirmation.
/// For v2, action_result carries state deltas (parsed as StateUpdatePayload).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OkActionPayload {
    #[serde(default)] pub action: String,
    #[serde(default)] pub destination: Option<String>,
    #[serde(default)] pub base: Option<String>,
    #[serde(default)] pub system: Option<String>,
    #[serde(default)] pub target: Option<String>,
    #[serde(default)] pub message: String,
}

/// `action_error` push event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActionErrorPayload {
    #[serde(default)] pub command: String,
    #[serde(default)] pub message: String,
    #[serde(default)] pub code: String,
}

// === Top-level WS frame envelope ===
// v2 framing: {"type":"<type>","request_id":"...","payload":{...}}
// Auth frames (welcome/logged_in) and push events share this envelope.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WsFrame {
    #[serde(default)] pub r#type: String,
    #[serde(default)] pub request_id: Option<String>,
    #[serde(default)] pub payload: serde_json::Value, // raw — route by `type` then parse
}

// === Error response (MCP / WS) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ErrorResponse {
    #[serde(default)] pub code: i64,
    #[serde(default)] pub message: String,
}

// === MCP tool call result (content[0].text is JSON-encoded game data) ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpToolResult {
    #[serde(default)]
    pub content: Vec<McpContentItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpContentItem {
    #[serde(default)] pub r#type: String,
    #[serde(default)] pub text: String, // JSON-encoded game data — parse twice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_status_deserializes_from_live_shape() {
        let json = r#"{
            "player": {
                "id": "e033f031a0c0cbb6abb21a684471b700",
                "username": "Drift",
                "empire": "nebula",
                "credits": 182636,
                "current_system": "sys_0459",
                "current_poi": "sys_0459_sun",
                "home_base": "haven_base"
            },
            "ship": {
                "id": "1f5d1e54",
                "name": "Deeprock Harvester",
                "hull": 346,
                "max_hull": 350,
                "shield": 100,
                "max_shield": 100,
                "fuel": 130,
                "max_fuel": 200,
                "cargo_capacity": 400
            }
        }"#;
        let result: PlayerStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.player.username, "Drift");
        assert_eq!(result.player.credits, 182636);
        assert_eq!(result.ship.name, "Deeprock Harvester");
        assert!((result.ship.hull_percent() - 98.86).abs() < 0.1);
    }

    #[test]
    fn player_status_handles_missing_fields() {
        let json = r#"{"player": {"username": "Test"}, "ship": {}}"#;
        let result: PlayerStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.player.username, "Test");
        assert_eq!(result.player.credits, 0);
        assert_eq!(result.ship.hull, 0);
    }

    #[test]
    fn nearby_response_handles_null_pirates() {
        let json = r#"{"count": 0, "nearby": [], "pirate_count": 0, "pirates": [], "poi_id": "sys_0459_sun"}"#;
        let result: NearbyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.count, 0);
        assert!(result.nearby.is_empty());
        assert!(result.pirates.is_empty());
    }

    #[test]
    fn cargo_response_deserializes() {
        let json = r#"{"available": 400, "capacity": 400, "used": 0, "cargo": [{"item_id": "ore_uranium", "quantity": 41, "size": 2}]}"#;
        let result: CargoResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.available, 400);
        assert_eq!(result.cargo.len(), 1);
        assert_eq!(result.cargo[0].item_id, "ore_uranium");
        assert_eq!(result.cargo[0].quantity, 41);
    }

    #[test]
    fn system_response_deserializes() {
        let json = r#"{
            "system": {"id": "sys_0459", "name": "GSC-0013", "description": "", "police_level": 0, "connections": ["sys_0319"], "pois": ["sys_0459_sun"], "position": {"x": 3815, "y": 3216}},
            "pois": [{"id": "sys_0459_sun", "system_id": "sys_0459", "type": "sun", "name": "Sun", "position": {"x": 0, "y": 0}}],
            "security_status": "Lawless (no police protection)"
        }"#;
        let result: SystemResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.system.name, "GSC-0013");
        assert_eq!(result.system.position.x, 3815);
        assert_eq!(result.pois.len(), 1);
        assert_eq!(result.pois[0].r#type, "sun");
    }

    #[test]
    fn missions_response_handles_null_missions() {
        let json = r#"{"missions": null, "total_count": 0, "max_missions": 5}"#;
        let result: MissionsResponse = serde_json::from_str(json).unwrap();
        assert!(result.missions.is_none());
        assert_eq!(result.max_missions, 5);
    }

    #[test]
    fn missions_response_deserializes_active_mission() {
        let json = r#"{
            "missions": [{
                "id": "m1", "type": "mining", "title": "Iron Supply Run",
                "description": "Mine iron", "difficulty": 1,
                "objectives": [{"description": "Mine 30 iron ore", "current": 15, "required": 30, "completed": false}],
                "rewards": {"credits": 1500, "skill_xp": {"mining_basic": 15}},
                "expires_at": "2026-02-12T00:00:00Z", "ticks_remaining": 8640
            }],
            "total_count": 1, "max_missions": 5
        }"#;
        let result: MissionsResponse = serde_json::from_str(json).unwrap();
        let missions = result.missions.unwrap();
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].title, "Iron Supply Run");
        assert_eq!(missions[0].objectives.len(), 1);
        assert_eq!(missions[0].objectives[0].current, 15);
        assert_eq!(missions[0].rewards.credits, 1500);
    }

    #[test]
    fn ws_frame_envelope_parses_welcome() {
        let json = r#"{"type":"welcome","payload":{"version":"0.530.1","tick_rate":10,"current_tick":49443}}"#;
        let frame: WsFrame = serde_json::from_str(json).unwrap();
        assert_eq!(frame.r#type, "welcome");
        assert!(frame.request_id.is_none());
        assert!(frame.payload.is_object());
    }

    #[test]
    fn ws_frame_envelope_parses_action_result_with_request_id() {
        let json = r#"{"type":"action_result","request_id":"abc123","payload":{"ship":{"hull":340}}}"#;
        let frame: WsFrame = serde_json::from_str(json).unwrap();
        assert_eq!(frame.r#type, "action_result");
        assert_eq!(frame.request_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn game_state_default_is_empty() {
        let state = GameState::default();
        assert_eq!(state.tick, 0);
        assert_eq!(state.player.username, "");
        assert_eq!(state.connection_state, ConnectionState::Disconnected);
        assert!(!state.in_combat);
    }

    #[test]
    fn connection_state_serializes_snake_case() {
        let s = serde_json::to_string(&ConnectionState::SessionReplaced).unwrap();
        assert_eq!(s, "\"session_replaced\"");
        let s = serde_json::to_string(&ConnectionState::RateLimited { retry_after: 30 }).unwrap();
        assert_eq!(s, "{\"rate_limited\":{\"retry_after\":30}}");
    }
}