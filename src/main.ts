// SpaceMolt Viewer — frontend entry point.
//
// v1 architecture (Decision #6): direct DOM mutation. No React, no reactive
// store, no virtual DOM. The Rust backend is the single source of truth; this
// file is a thin renderer that subscribes to Tauri events and rewrites the
// relevant DOM nodes.
//
// Tauri event channels (emitted by Rust):
//   state_update      → full per-account game state snapshot
//   game_event        → single event-feed item (category, title, detail)
//   connection_state  → per-account connection status change
//   chat_message      → inbound chat line
//
// Tauri commands (frontend → Rust):
//   connect_account(username, password) → AccountId
//   disconnect_account(AccountId)
//   add_account(display_name, username, password)
//   get_skills(AccountId) → Skills JSON
//
// v1 layout: 3 panes (accounts | monitor | logs). See Task 12 for the full
// layout; this file wires the event listeners and a minimal boot screen.

console.log("SpaceMolt Viewer frontend loaded");

// Placeholder: Task 11/12 will flesh out the event listeners and DOM
// rendering. For Task 1 we only verify the bundle loads.
const boot = document.getElementById("boot");
if (boot) {
  boot.textContent = "SpaceMolt Viewer — frontend ready (v0.1.0)";
}