# SpaceMolt Viewer (Windows)

A Windows-native, multi-account dashboard for [SpaceMolt](https://www.spacemolt.com/) — the AI agent MMO. Ports the macOS [SpaceMoltViewer](https://github.com/pj4533/SpaceMoltViewer) to Tauri v2 with modernized protocol (WS v2 + MCP `rmcp` SDK) and multi-account support.

## Status

🚧 **In development — Task 1 (scaffold)**. See `projects/spacemolt/plans/2026-07-19-spacemolt-viewer-windows.md` in the Obsidian vault for the full grilled plan.

## Tech Stack

- **Backend:** Rust (tokio, tokio-tungstenite, `rmcp`, serde, tracing)
- **Frontend:** Vanilla TypeScript + Vite, Tailwind CSS (direct DOM mutation — no React)
- **Framework:** Tauri v2
- **Credentials:** `tauri-plugin-stronghold` (encrypted at rest, cross-platform)

## Prerequisites (Windows)

- [Rust](https://rustup.rs/) (stable, `x86_64-pc-windows-msvc` target)
- [Visual Studio 2022](https://visualstudio.microsoft.com/) with "Desktop development with C++" workload (provides the MSVC linker)
- [Node.js](https://nodejs.org/) 18+
- WebView2 Runtime (pre-installed on Windows 10 1803+ / Windows 11)

## Development

```bash
npm install
npm run tauri dev
```

## Architecture (v1 — 3-pane dashboard)

```
+-------------------------------------------------------------------+
|  Toolbar                                                          |
+-------------------+---------------------------+-------------------+
|   ACCOUNTS        |   MONITOR                 |   LOGS            |
|   (left)          |   (center)                |   (right)         |
|                   |                           |                   |
|   Account list    |   Player / ship / system  |   MCP + network   |
|   + status        |   state for selected      |   event feed      |
|                   |   account                 |                   |
+-------------------+---------------------------+-------------------+
```

See the plan for v2 scope (galaxy map, inspector, activity bar).

## Safety

This is a **read-only viewer**. A 71-tool safety allowlist (verified against the live v0.530.1 snapshot) blocks all mutation endpoints — the viewer can never interfere with an active bot session.

## License

MIT