// SpaceMolt Viewer — TypeScript type definitions.
//
// These mirror the Rust serde models in src-tauri/src/models.rs.
// All fields are optional (default to falsy values) because the Rust side
// uses `#[serde(default)]` — the frontend should handle missing fields
// gracefully (the game server sends inconsistent shapes).

// === Account ===

export interface AccountInfo {
  display_name: string;
  username: string;
}

// === Player ===

export interface Player {
  id: string;
  username: string;
  empire: string;
  credits: number;
  created_at: string;
  last_login_at: string;
  last_active_at: string;
  status_message: string;
  clan_tag: string;
  primary_color: string;
  secondary_color: string;
  anonymous: boolean;
  is_cloaked: boolean;
  current_ship_id: string;
  current_system: string;
  current_poi: string;
  home_base: string;
  docked_at_base: string;
  skills: Record<string, number>;
  skill_xp: Record<string, number>;
  experience: number;
  stats: PlayerStats;
  discovered_systems: Record<string, boolean>;
}

export interface PlayerStats {
  credits_earned: number;
  credits_spent: number;
  ships_destroyed: number;
  ships_lost: number;
  pirates_destroyed: number;
  ore_mined: number;
  items_crafted: number;
  trades_completed: number;
  systems_explored: number;
}

// === Ship ===

export interface ShipOverview {
  id: string;
  name: string;
  class_id: string;
  hull: number;
  max_hull: number;
  shield: number;
  max_shield: number;
  armor: number;
  fuel: number;
  max_fuel: number;
  speed: number;
  cargo_used: number;
  cargo_capacity: number;
  cpu_used: number;
  cpu_max: number;
  power_used: number;
  power_max: number;
}

// === Game state (emitted as state_update event) ===

export interface GameState {
  tick: number;
  player: Player;
  ship: ShipOverview;
  nearby: NearbyPlayer[];
  pirates: Pirate[];
  in_combat: boolean;
  travel_progress: number | null;
  travel_destination: string | null;
  current_system: StarSystem;
  current_poi: PointOfInterest;
  cargo: CargoResponse;
  skills: SkillsResponse;
  missions: MissionsResponse;
  connection_state: ConnectionState;
}

export interface NearbyPlayer {
  username: string;
  display_name: string;
  empire: string;
  ship_class: string;
  distance: number;
  is_hostile: boolean;
  is_docked: boolean;
}

export interface Pirate {
  id: string;
  name: string;
  level: number;
  ship_class: string;
  distance: number;
}

export interface StarSystem {
  id: string;
  name: string;
  x: number;
  y: number;
  security: string;
  station_count: number;
}

export interface PointOfInterest {
  id: string;
  name: string;
  type: string;
  system_id: string;
}

export interface CargoResponse {
  available: number;
  capacity: number;
  used: number;
  cargo: CargoItem[];
}

export interface CargoItem {
  item_id: string;
  quantity: number;
}

export interface SkillsResponse {
  skills: Record<string, number>;
}

export interface MissionsResponse {
  missions: Mission[];
}

export interface Mission {
  id: string;
  title: string;
  description: string;
  reward: number;
  status: string;
}

// === Connection state (emitted as connection_state event) ===

export type ConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'session_expired'
  | 'session_replaced'
  | { rate_limited: { retry_after: number } }
  | 'error'
  | 'offline';

// === Game event feed (emitted as game_event event) ===

export type GameEventCategory =
  | 'info'
  | 'combat'
  | 'mining'
  | 'navigation'
  | 'trade'
  | 'skill'
  | 'pirate'
  | 'scan'
  | 'base'
  | 'system'
  | 'broadcast';

export interface GameEvent {
  category: GameEventCategory;
  title: string;
  detail: string;
  timestamp: number; // unix millis
}

// === Tauri event payload wrappers (from game_state.rs) ===
// Each event includes the account username for routing to the correct pane.

export interface AccountStateUpdate {
  username: string;
  state: GameState;
}

export interface AccountGameEvent {
  username: string;
  event: GameEvent;
}

export interface AccountConnectionState {
  username: string;
  state: ConnectionState;
}