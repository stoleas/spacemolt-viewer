# SpaceMolt Viewer

A native Windows desktop dashboard for [SpaceMolt](https://www.spacemolt.com), an AI agent MMO.
Built with Tauri v2 (Rust backend + Vanilla TypeScript frontend).

## Features

- **Multi-account support** — Monitor multiple accounts simultaneously with per-account state, events, and connection status
- **Dual-channel transport** — WebSocket v2 for push events + MCP (JSON-RPC) for queries
- **3-pane layout** — Accounts sidebar, system monitor, and event feed
- **Real-time game state** — Hull/shield/fuel/cargo gauges, nearby players, pirate warnings, combat alerts, travel progress
- **Event feed** — Color-coded by category (combat, mining, navigation, trade, skill, pirate, scan, broadcast)
- **Auto-reconnect** — Close-code-aware state machine: `1000`→backoff, `4001`→abort, `4002`→immediate, `4003`→parse `retry_after`
- **Secure credentials** — Encrypted at rest via [Stronghold](https://github.com/iotaledger/stronghold.rs) (IOTA)
- **Auto-connect** — First saved account connects automatically on launch
- **Read-only** — 71-tool safety allowlist blocks all mutation endpoints

## Requirements

- Windows 10 (1803+) or Windows 11
- WebView2 (pre-installed on Windows 11, auto-installs on Windows 10)
- No additional runtime dependencies

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (stable) — install on Windows side
- [Node.js](https://nodejs.org/) 18+
- [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) ("Desktop development with C++" workload)

### Build & Run

```bash
npm install
npm run tauri dev
```

### Build for release

```bash
npm run tauri build
```

Produces an MSI installer in `src-tauri/target/release/bundle/`.

### Building from WSL

The Rust toolchain runs on the Windows side. From WSL:

```bash
cd /home/equail/code/spacemolt-viewer/src-tauri
/mnt/c/Users/equail/.cargo/bin/cargo.exe check   # type check
/mnt/c/Users/equail/.cargo/bin/cargo.exe test     # run tests (112 tests)
```

Frontend (TypeScript + Vite) can be checked from WSL:

```bash
npx tsc --noEmit          # type check
npx vite build             # production build (run via PowerShell on Windows)
```

## Setup

1. Launch the app
2. Create a master password (encrypts your stored game credentials)
3. Click "+" to add a SpaceMolt account (display name, username, password)
4. The app connects via WebSocket + MCP and begins receiving live game state

On future launches, enter your master password to unlock credentials. The first saved account auto-connects.

## Architecture

### Backend (Rust — `src-tauri/src/`)

| Module | Responsibility |
|--------|---------------|
| `models.rs` | Serde data models with `#[serde(default)]` for lenient deserialization |
| `mcp_client.rs` | MCP JSON-RPC client using official `rmcp` SDK + session recovery |
| `ws_client.rs` | WebSocket v2 client with close-code-aware auto-reconnect |
| `game_api.rs` | Typed query methods + 71-tool safety allowlist |
| `credential_store.rs` | Stronghold-based encrypted credential storage |
| `account_manager.rs` | Multi-account manager with persistent config |
| `account_session.rs` | Per-account connection lifecycle (WS + MCP + GSM handles) |
| `game_state.rs` | GameStateManager: push event processing, v2 state delta merging, periodic refresh |
| `commands.rs` | Tauri IPC commands (frontend → backend) |
| `lib.rs` | App entry point, module wiring, Tauri builder |

### Frontend (TypeScript — `src/`)

| File | Responsibility |
|------|---------------|
| `main.ts` | Event listeners, DOM rendering, IPC wiring, boot sequence |
| `types.ts` | TypeScript type definitions mirroring Rust serde models |
| `styles.css` | Dark theme, 3-pane grid layout, gauges, badges, modals |
| `index.html` | 3-pane layout + modal templates |

**Design**: Direct DOM mutation — no React, no virtual DOM, no reactive store. The Rust backend is the single source of truth; the frontend is a thin renderer that subscribes to Tauri events and rewrites DOM nodes.

### Communication

The app connects to the SpaceMolt game server via:

- **WebSocket v2** (`wss://game.spacemolt.com/ws/v2`) — push-only: state updates, events, notifications
- **MCP** (`https://game.spacemolt.com/mcp`) — on-demand queries via `rmcp` SDK: `get_state`, `get_map`, `get_notifications`, etc.

All queries go through a 71-tool safety allowlist — only read-only tools are permitted.

### Tauri IPC

**Commands** (frontend → backend):
- `credentials_initialized`, `init_credentials`, `load_accounts`
- `get_accounts`, `add_account`, `remove_account`, `update_password`
- `connect_account`, `disconnect_account`
- `is_account_connected`, `list_connected_accounts`

**Events** (backend → frontend):
- `state_update` — full game state for an account
- `game_event` — single event feed item
- `connection_state` — per-account connection status change

## Testing

```bash
cd src-tauri
cargo test
```

112 tests covering: serde deserialization, MCP client, WS client reconnect logic, game state processing, account session lifecycle, credential store, and account manager.

## License

Private project.