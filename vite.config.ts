import { defineConfig } from "vite";

export default defineConfig({
  // Tauri expects a fixed port and external host; if the frontend is served
  // from a different host, use `tauri dev`'s `--host`/`--port` or set these.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "0.0.0.0",
    hmr: {
      protocol: "ws",
      host: "localhost",
      port: 1421,
    },
  },
  // Env variables starting with TAURI_ are exposed to the frontend by Tauri.
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // Tauri supports macOS/Linux/Windows; keep targetDir per-platform.
    targetDir: "dist",
    // Tauri webviews are modern Chromium — esbuild minify is fine.
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});