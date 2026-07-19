// SpaceMolt Viewer — Rust backend entry point.
//
// Tauri v2 app. v1 scope (per grilling Decision #2):
//   - 3-pane frontend (accounts / monitor / logs)
//   - Multi-account via HashMap<String, AccountSession> (Decision #5)
//   - Dual-channel networking: WS v2 push + MCP queries via rmcp (Decisions #1, #4, #8)
//   - Credential storage via tauri-plugin-stronghold (Decision #3)
//
// This file wires up the Tauri app, registers plugins, and delegates to
// `commands::run()` which sets up state and registers Tauri commands. Task 1
// only needs a minimal app that boots; later tasks fill in state and commands.

use tracing_subscriber::EnvFilter;

/// Initializes the tracing subscriber with `RUST_LOG` env var support.
/// Called once at startup.
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

/// Tauri app entry point. Task 1: minimal app that opens a window and
/// initializes tracing + Stronghold plugin. Later tasks register commands
/// and state via `tauri::Builder::manage(...)`.
pub fn run() {
    init_tracing();
    tracing::info!("SpaceMolt Viewer starting (v0.1.0)");

    // Stronghold plugin: the closure derives a vault-key hash from the
    // user-provided password. The actual vault password is supplied at runtime
    // via the frontend `load_store`/`create_store` IPC calls (Task 6).
    tauri::Builder::default()
        .plugin(
            tauri_plugin_stronghold::Builder::new(|password: &str| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                password.hash(&mut hasher);
                hasher.finish().to_le_bytes().to_vec()
            })
            .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}