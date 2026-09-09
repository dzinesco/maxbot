// v2.0.0 — top-level React ErrorBoundary. Catches any render
// error in the App tree and surfaces it on screen instead of
// showing a blank black window (which is what a top-level
// React crash looks like when no boundary catches it).
//
// Built for the v2.0 black-screen investigation: the
// hypothesis is that something in the App render path
// throws on first launch on Tyler's machine but renders
// fine on the orchestrator's. The ErrorBoundary doesn't
// fix the underlying bug, but it makes the error visible
// (and copyable) so we can debug from the user's side
// instead of guessing.

import { Component, type ReactNode } from "react";

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
  info: { componentStack?: string } | null;
}

export class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = { error: null, info: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error, info: null };
  }

  componentDidCatch(error: Error, info: { componentStack?: string }) {
    // Log to the console so it shows in the WebKit inspector
    // (right-click → Inspect Element when devtools are available).
    // eslint-disable-next-line no-console
    console.error(
      "[ErrorBoundary] caught render error:",
      error,
      info.componentStack ?? "",
    );
    this.setState({ error, info });
  }

  render() {
    const { error, info } = this.state;
    if (error === null) {
      return this.props.children;
    }
    return (
      <div
        style={{
          padding: 24,
          color: "#fafafa",
          background: "#18181b",
          height: "100vh",
          overflow: "auto",
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, monospace",
          fontSize: 13,
          lineHeight: 1.5,
        }}
        data-testid="error-boundary-fallback"
      >
        <h1 style={{ fontSize: 18, marginTop: 0, color: "#f87171" }}>
          MaxBot hit an error while starting
        </h1>
        <p style={{ color: "#a1a1aa" }}>
          Paste the error below to the developer — this is a render-time
          crash (likely from the v2.0 Slice E BotRoster / Sidebar
          refactor).
        </p>
        <h2 style={{ fontSize: 14, marginTop: 16, color: "#fafafa" }}>
          Error message
        </h2>
        <pre
          style={{
            background: "#0e0e10",
            padding: 12,
            borderRadius: 6,
            overflow: "auto",
            whiteSpace: "pre-wrap",
            margin: 0,
            color: "#fafafa",
          }}
        >
          {error.name}: {error.message}
        </pre>
        {info?.componentStack && (
          <>
            <h2 style={{ fontSize: 14, marginTop: 16, color: "#fafafa" }}>
              Component stack
            </h2>
            <pre
              style={{
                background: "#0e0e10",
                padding: 12,
                borderRadius: 6,
                overflow: "auto",
                whiteSpace: "pre-wrap",
                margin: 0,
                fontSize: 11,
                color: "#a1a1aa",
              }}
            >
              {info.componentStack}
            </pre>
          </>
        )}
        <h2 style={{ fontSize: 14, marginTop: 16, color: "#fafafa" }}>
          Build info
        </h2>
        <pre
          style={{
            background: "#0e0e10",
            padding: 12,
            borderRadius: 6,
            overflow: "auto",
            whiteSpace: "pre-wrap",
            margin: 0,
            fontSize: 11,
            color: "#a1a1aa",
          }}
        >
          {[
            `maxbot v${__APP_VERSION__ ?? "unknown"}`,
            `commit ${__GIT_COMMIT__ ?? "unknown"}`,
            `built ${__BUILD_TIME__ ?? "unknown"}`,
            `ua ${navigator.userAgent}`,
          ].join("\n")}
        </pre>
      </div>
    );
  }
}
