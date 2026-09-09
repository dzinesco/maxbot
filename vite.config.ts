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
  // The v2.0 Slice C `noVncViewer` brings in `@novnc/novnc`,
  // which uses a top-level `await` in `core/util/browser.js`
  // to probe `WebCodecs` H264 support. Tauri's default esbuild
  // target is `chrome87` (no top-level await), which causes
  // the build to fail with "Top-level await is not available".
  // The Tauri 2 webview minimums (macOS 12+, Windows WebView2,
  // Linux WebKitGTK 2.32+) all support top-level await in
  // modules, so we bump to `esnext` for the build target —
  // this only affects the build (the actual code is still
  // ES2022-ish), and the runtime behavior is unchanged.
  build: {
    target: "esnext",
  },
  esbuild: {
    target: "esnext",
  },
  optimizeDeps: {
    esbuildOptions: {
      target: "esnext",
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
