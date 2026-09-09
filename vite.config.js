var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
/// <reference types="vitest" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { execSync } from "node:child_process";
var host = process.env.TAURI_DEV_HOST;
// Build-time constants injected via `define`. The ErrorBoundary
// fallback surfaces these so the user can confirm what build
// they're running when reporting a v2.0 black-screen bug.
var gitCommit = (function () {
    try {
        return execSync("git rev-parse --short HEAD", { stdio: ["ignore", "pipe", "ignore"] })
            .toString()
            .trim();
    }
    catch (_a) {
        return "unknown";
    }
})();
var buildTime = new Date().toISOString();
// Vite config tuned for Tauri: fixed dev port, no opening a browser, HMR over
// the Tauri-side webview socket.
export default defineConfig(function () { return __awaiter(void 0, void 0, void 0, function () {
    return __generator(this, function (_a) {
        return [2 /*return*/, ({
                plugins: [react()],
                clearScreen: false,
                define: {
                    __APP_VERSION__: JSON.stringify("2.0.0"),
                    __GIT_COMMIT__: JSON.stringify(gitCommit),
                    __BUILD_TIME__: JSON.stringify(buildTime),
                },
                server: {
                    port: 1420,
                    strictPort: true,
                    host: host || false,
                    hmr: host
                        ? {
                            protocol: "ws",
                            host: host,
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
            })];
    });
}); });
