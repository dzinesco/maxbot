/// <reference types="vitest" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

// Vite config tuned for Tauri: fixed dev port, no opening a browser, HMR over
// the Tauri-side webview socket.
export default defineConfig(async () => ({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test-setup.ts"],
    // The Tauri `@tauri-apps/api/core` `invoke` is a no-op here
    // (no Tauri runtime), but any import chain that pulls in a
    // Tauri module still needs to evaluate without throwing —
    // vitest's module resolver handles that, and the components
    // we test don't call into the Tauri APIs at all.
    globals: false,
  },
}));
