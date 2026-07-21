// SpaceMolt Viewer — frontend entry point.
//
// v1 architecture (Decision #6): direct DOM mutation. No React, no reactive
// store, no virtual DOM. The Rust backend is the single source of truth; this
// file is a thin renderer that subscribes to Tauri events and rewrites the
// relevant DOM nodes.
//
// Tauri events (emitted by Rust game_state.rs):
//   state_update       → { username, state: GameState }
//   game_event         → { username, event: GameEvent }
//   connection_state   → { username, state: ConnectionState }
//
// Tauri commands (frontend → Rust commands.rs):
//   credentials_initialized() → boolean
//   init_credentials(password) → void
//   load_accounts(password) → void
//   get_accounts() → AccountInfo[]
//   add_account(display_name, username, password) → void
//   remove_account(username) → void
//   update_password(username, new_password) → void
//   connect_account(username) → void
//   disconnect_account(username) → void
//   is_account_connected(username) → boolean
//   list_connected_accounts() → string[]

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AccountInfo,
  AccountConnectionState,
  AccountGameEvent,
  AccountStateUpdate,
  ConnectionState,
  GameEvent,
  GameState,
} from "./types";

// === State ===

let accounts: AccountInfo[] = [];
let activeUsername: string | null = null;
const perAccountState = new Map<string, GameState>();
const perAccountConnState = new Map<string, ConnectionState>();
const perAccountEvents = new Map<string, GameEvent[]>();
const unlisteners: UnlistenFn[] = [];

// === DOM helpers ===

function el<T extends HTMLElement>(id: string): T {
  const e = document.getElementById(id);
  if (!e) throw new Error(`Element #${id} not found`);
  return e as T;
}

function clearChildren(node: HTMLElement): void {
  node.innerHTML = "";
}

function formatConnState(state: ConnectionState): string {
  if (typeof state === "object" && state !== null) {
    if ("rate_limited" in state) {
      return `Rate Limited (retry in ${state.rate_limited.retry_after}s)`;
    }
  }
  switch (state) {
    case "disconnected": return "Disconnected";
    case "connecting": return "Connecting…";
    case "connected": return "Connected";
    case "reconnecting": return "Reconnecting…";
    case "session_expired": return "Session Expired";
    case "session_replaced": return "Session Replaced";
    case "error": return "Error";
    case "offline": return "Offline";
    default: return String(state);
  }
}

function connStateClass(state: ConnectionState): string {
  if (typeof state === "object") return "error";
  switch (state) {
    case "disconnected": return "disconnected";
    case "connecting": return "connecting";
    case "connected": return "connected";
    case "reconnecting": return "reconnecting";
    case "session_expired":
    case "session_replaced":
    case "error":
    case "offline":
      return "error";
    default: return "disconnected";
  }
}

function formatTime(ts: number): string {
  if (!ts) return "--:--:--";
  const d = new Date(ts);
  return d.toLocaleTimeString("en-US", { hour12: false });
}

function gauge(value: number, max: number, cls: string): string {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return `<div class="gauge"><div class="gauge-fill ${cls}" style="width:${pct}%"></div></div>`;
}

// === Rendering ===

function renderAccountList(): void {
  const list = el("account-list");
  clearChildren(list);

  if (accounts.length === 0) {
    list.innerHTML = '<div class="empty-state">No accounts configured.<br>Click + to add one.</div>';
    return;
  }

  for (const acct of accounts) {
    const connState = perAccountConnState.get(acct.username) ?? "disconnected";
    const isActive = acct.username === activeUsername;

    const item = document.createElement("div");
    item.className = `account-item${isActive ? " active" : ""}`;
    item.dataset.username = acct.username;
    item.innerHTML = `
      <div class="account-dot ${connStateClass(connState)}"></div>
      <div class="account-name">${escapeHtml(acct.display_name)}</div>
      <div class="account-actions">
        ${connState === "connected" || connState === "connecting" || connState === "reconnecting"
          ? `<button class="btn-disconnect" data-action="disconnect" title="Disconnect">■</button>`
          : `<button class="btn-connect" data-action="connect" title="Connect">▶</button>`}
        <button data-action="remove" title="Remove account">✕</button>
      </div>
    `;

    // Click on the item (not the buttons) selects the account
    item.addEventListener("click", (ev) => {
      const target = ev.target as HTMLElement;
      if (target.tagName === "BUTTON") return;
      selectAccount(acct.username);
    });

    // Action buttons
    item.querySelectorAll("button[data-action]").forEach((btn) => {
      btn.addEventListener("click", (ev) => {
        ev.stopPropagation();
        const action = (btn as HTMLElement).dataset.action;
        if (action === "connect") {
          invoke("connect_account", { username: acct.username }).catch(showError);
        } else if (action === "disconnect") {
          invoke("disconnect_account", { username: acct.username }).catch(showError);
        } else if (action === "remove") {
          if (confirm(`Remove account "${acct.display_name}"?`)) {
            invoke("remove_account", { username: acct.username })
              .then(() => refreshAccounts())
              .catch(showError);
          }
        }
      });
    });

    list.appendChild(item);
  }
}

function renderMonitor(): void {
  const content = el("monitor-content");
  const nameEl = el("active-account-name");

  if (!activeUsername) {
    nameEl.textContent = "No account selected";
    clearChildren(content);
    content.innerHTML = '<div class="empty-state">Select an account to view its status.</div>';
    return;
  }

  const acct = accounts.find((a) => a.username === activeUsername);
  nameEl.textContent = acct?.display_name ?? activeUsername;

  const state = perAccountState.get(activeUsername);
  const connState = perAccountConnState.get(activeUsername) ?? "disconnected";

  if (!state) {
    clearChildren(content);
    content.innerHTML = `
      <div class="empty-state">
        <div class="conn-badge ${connStateClass(connState)}">${formatConnState(connState)}</div>
        <p style="margin-top:1rem">Waiting for game state…</p>
      </div>`;
    return;
  }

  const { player, ship, current_system, current_poi } = state;
  clearChildren(content);

  const grid = document.createElement("div");
  grid.className = "monitor-grid";
  grid.innerHTML = `
    <div class="monitor-card" style="grid-column: 1 / -1">
      <h3>Connection</h3>
      <span class="conn-badge ${connStateClass(connState)}">${formatConnState(connState)}</span>
    </div>

    <div class="monitor-card">
      <h3>Player</h3>
      <div class="value">${escapeHtml(player.username)}</div>
      <div class="sub">${escapeHtml(player.empire || "No empire")} · ${player.credits.toLocaleString()} credits</div>
      <div class="sub">${escapeHtml(current_system.name || "Unknown system")}${current_poi.name ? " · " + escapeHtml(current_poi.name) : ""}</div>
    </div>

    <div class="monitor-card">
      <h3>Ship</h3>
      <div class="value">${escapeHtml(ship.name || "No ship")}</div>
      <div class="sub">${escapeHtml(ship.class_id || "unknown class")}</div>
    </div>

    <div class="monitor-card">
      <h3>Hull</h3>
      <div class="value">${ship.hull} / ${ship.max_hull}</div>
      ${gauge(ship.hull, ship.max_hull, "hull")}
    </div>

    <div class="monitor-card">
      <h3>Shield</h3>
      <div class="value">${ship.shield} / ${ship.max_shield}</div>
      ${gauge(ship.shield, ship.max_shield, "shield")}
    </div>

    <div class="monitor-card">
      <h3>Fuel</h3>
      <div class="value">${ship.fuel} / ${ship.max_fuel}</div>
      ${gauge(ship.fuel, ship.max_fuel, "fuel")}
    </div>

    <div class="monitor-card">
      <h3>Cargo</h3>
      <div class="value">${ship.cargo_used} / ${ship.cargo_capacity}</div>
      ${gauge(ship.cargo_used, ship.cargo_capacity, "cargo")}
    </div>

    ${state.in_combat ? `
    <div class="monitor-card" style="grid-column: 1 / -1; border-color: #ef4444">
      <h3 style="color: #ef4444">⚠ In Combat</h3>
    </div>` : ""}

    ${state.travel_destination ? `
    <div class="monitor-card" style="grid-column: 1 / -1">
      <h3>Traveling</h3>
      <div class="value">${escapeHtml(state.travel_destination)}</div>
      <div class="sub">${state.travel_progress != null ? Math.round(state.travel_progress * 100) + "%" : "In transit"}</div>
    </div>` : ""}

    ${state.nearby.length > 0 ? `
    <div class="monitor-card" style="grid-column: 1 / -1">
      <h3>Nearby Players (${state.nearby.length})</h3>
      ${state.nearby.slice(0, 5).map((p) => `
        <div class="sub">${escapeHtml(p.username)} · ${escapeHtml(p.ship_class)} · ${Math.round(p.distance)}m${p.is_hostile ? ' · <span style="color:#ef4444">HOSTILE</span>' : ""}</div>
      `).join("")}
    </div>` : ""}

    ${state.pirates.length > 0 ? `
    <div class="monitor-card" style="grid-column: 1 / -1; border-color: #f97316">
      <h3 style="color: #f97316">⚠ Pirates (${state.pirates.length})</h3>
      ${state.pirates.slice(0, 5).map((p) => `
        <div class="sub">${escapeHtml(p.name)} · Level ${p.level} · ${escapeHtml(p.ship_class)} · ${Math.round(p.distance)}m</div>
      `).join("")}
    </div>` : ""}
  `;
  content.appendChild(grid);
}

function renderEvents(): void {
  const list = el("event-list");
  clearChildren(list);

  if (!activeUsername) {
    list.innerHTML = '<div class="empty-state">No events yet.</div>';
    return;
  }

  const events = perAccountEvents.get(activeUsername);
  if (!events || events.length === 0) {
    list.innerHTML = '<div class="empty-state">No events yet.</div>';
    return;
  }

  for (const ev of events) {
    const item = document.createElement("div");
    item.className = `event-item ${ev.category}`;
    item.innerHTML = `
      <div class="event-time">${formatTime(ev.timestamp)}</div>
      <div class="event-title">${escapeHtml(ev.title)}</div>
      ${ev.detail ? `<div class="event-detail">${escapeHtml(ev.detail)}</div>` : ""}
    `;
    list.appendChild(item);
  }
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function showError(err: unknown): void {
  const msg = err instanceof Error ? err.message : String(err);
  console.error("[SpaceMolt]", msg);
  // Show in the event list for visibility
  if (activeUsername) {
    appendEventLocal(activeUsername, "error", "Error", msg);
    renderEvents();
  }
}

// === Account management ===

function selectAccount(username: string): void {
  activeUsername = username;
  renderAccountList();
  renderMonitor();
  renderEvents();
}

async function refreshAccounts(): Promise<void> {
  try {
    accounts = await invoke<AccountInfo[]>("get_accounts");
    renderAccountList();
    // If the active account was removed, deselect
    if (activeUsername && !accounts.find((a) => a.username === activeUsername)) {
      activeUsername = null;
      renderMonitor();
      renderEvents();
    }
  } catch (err) {
    showError(err);
  }
}

function appendEventLocal(username: string, category: string, title: string, detail: string): void {
  let events = perAccountEvents.get(username);
  if (!events) {
    events = [];
    perAccountEvents.set(username, events);
  }
  events.unshift({
    category: category as GameEvent["category"],
    title,
    detail,
    timestamp: Date.now(),
  });
  if (events.length > 200) events.length = 200;
}

// === Event listeners ===

async function setupEventListeners(): Promise<void> {
  // state_update — full game state for an account
  unlisteners.push(
    await listen<AccountStateUpdate>("state_update", (e) => {
      const { username, state } = e.payload;
      perAccountState.set(username, state);
      // Also update connection state from the game state
      perAccountConnState.set(username, state.connection_state);
      if (username === activeUsername) {
        renderMonitor();
      }
      renderAccountList(); // Update connection dot
    }),
  );

  // game_event — single event feed item
  unlisteners.push(
    await listen<AccountGameEvent>("game_event", (e) => {
      const { username, event } = e.payload;
      let events = perAccountEvents.get(username);
      if (!events) {
        events = [];
        perAccountEvents.set(username, events);
      }
      events.unshift(event);
      if (events.length > 200) events.length = 200;
      if (username === activeUsername) {
        renderEvents();
      }
    }),
  );

  // connection_state — per-account connection status change
  unlisteners.push(
    await listen<AccountConnectionState>("connection_state", (e) => {
      const { username, state } = e.payload;
      perAccountConnState.set(username, state);
      if (username === activeUsername) {
        renderMonitor();
      }
      renderAccountList();
    }),
  );
}

// === Modal management ===

function showModal(id: string): void {
  el(id).classList.remove("hidden");
}

function hideModal(id: string): void {
  el(id).classList.add("hidden");
}

function setupAddAccountModal(): void {
  const btnAdd = el("btn-add-account");
  const btnCancel = el("btn-cancel-add");
  const form = el("add-account-form") as HTMLFormElement;
  const errEl = el("add-account-error");

  btnAdd.addEventListener("click", () => showModal("add-account-modal"));
  btnCancel.addEventListener("click", () => hideModal("add-account-modal"));

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const displayName = (el("input-display-name") as HTMLInputElement).value.trim();
    const username = (el("input-username") as HTMLInputElement).value.trim();
    const password = (el("input-password") as HTMLInputElement).value;

    if (!displayName || !username || !password) {
      errEl.textContent = "All fields are required.";
      errEl.classList.remove("hidden");
      return;
    }

    errEl.classList.add("hidden");
    try {
      await invoke("add_account", {
        displayName,
        username,
        password,
      });
      await refreshAccounts();
      hideModal("add-account-modal");
      form.reset();
    } catch (err) {
      errEl.textContent = err instanceof Error ? err.message : String(err);
      errEl.classList.remove("hidden");
    }
  });
}

function setupCredentialModals(): void {
  const setupForm = el("credential-setup-form") as HTMLFormElement;
  const setupErr = el("credential-setup-error");
  const unlockForm = el("credential-unlock-form") as HTMLFormElement;
  const unlockErr = el("credential-unlock-error");

  setupForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const password = (el("input-master-password") as HTMLInputElement).value;
    if (!password) {
      setupErr.textContent = "Password is required.";
      setupErr.classList.remove("hidden");
      return;
    }
    setupErr.classList.add("hidden");
    try {
      await invoke("init_credentials", { password });
      hideModal("credential-setup-modal");
      await refreshAccounts();
      await autoConnectFirst();
    } catch (err) {
      setupErr.textContent = err instanceof Error ? err.message : String(err);
      setupErr.classList.remove("hidden");
    }
  });

  unlockForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const password = (el("input-unlock-password") as HTMLInputElement).value;
    if (!password) {
      unlockErr.textContent = "Password is required.";
      unlockErr.classList.remove("hidden");
      return;
    }
    unlockErr.classList.add("hidden");
    try {
      await invoke("load_accounts", { password });
      hideModal("credential-unlock-modal");
      await refreshAccounts();
      await autoConnectFirst();
    } catch (err) {
      unlockErr.textContent = err instanceof Error ? err.message : String(err);
      unlockErr.classList.remove("hidden");
    }
  });
}

// === Clear events button ===

function setupClearEvents(): void {
  el("btn-clear-events").addEventListener("click", () => {
    if (activeUsername) {
      perAccountEvents.delete(activeUsername);
      renderEvents();
    }
  });
}

// === Auto-connect on launch (Decision #10) ===
// Auto-connect the first saved account after credentials are unlocked.
// Uses the serial connection queue — no parallel connects.

let autoConnectAttempted = false;

async function autoConnectFirst(): Promise<void> {
  if (autoConnectAttempted) return;
  if (accounts.length === 0) return;
  autoConnectAttempted = true;

  const first = accounts[0];
  selectAccount(first.username);
  info(`Auto-connecting first account: ${first.username}`);
  try {
    await invoke("connect_account", { username: first.username });
  } catch (err) {
    // Non-fatal — user can manually connect via the ▶ button
    console.error("[SpaceMolt] Auto-connect failed:", err);
  }
}

function info(msg: string): void {
  console.log(`[SpaceMolt] ${msg}`);
}

// === Boot sequence ===

async function boot(): Promise<void> {
  console.log("SpaceMolt Viewer frontend loaded");

  // Set up event listeners first so we don't miss any events
  await setupEventListeners();

  // Wire up UI controls
  setupAddAccountModal();
  setupCredentialModals();
  setupClearEvents();

  // Check if credentials are already initialized
  try {
    const initialized = await invoke<boolean>("credentials_initialized");
    if (initialized) {
      // Credentials exist — show unlock modal
      showModal("credential-unlock-modal");
    } else {
      // First launch — show setup modal
      showModal("credential-setup-modal");
    }
  } catch (err) {
    console.error("Failed to check credential status:", err);
    // Show setup modal as fallback
    showModal("credential-setup-modal");
  }

  renderAccountList();
  renderMonitor();
  renderEvents();
}

// Run boot sequence
boot().catch(console.error);

// Cleanup event listeners on page unload
window.addEventListener("beforeunload", () => {
  for (const unlisten of unlisteners) {
    unlisten();
  }
});