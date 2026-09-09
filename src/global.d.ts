// v2.0.0 — build-time constants injected by Vite's `define` block
// (see vite.config.js). These are baked into the bundle at build
// time so the ErrorBoundary can surface them when a render error
// is caught — useful for the v2.0 black-screen investigation.
declare const __APP_VERSION__: string;
declare const __GIT_COMMIT__: string;
declare const __BUILD_TIME__: string;
