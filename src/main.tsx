// v3.7.17 — v4 shell entry point.
//
// Per the Slice 4 brief (ui/v4-shell):
// - No React.StrictMode in production. StrictMode's double-render
//   hides timer / listener leaks that this slice is meant to expose.
// - ErrorBoundary stays: top-level render errors must show on screen
//   instead of producing a blank black window.
// - The global stylesheet (./styles.css) is the OLD tree — v4
//   surfaces import only their own per-surface stylesheets.

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./v4/App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./v4/styles/base.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <ErrorBoundary>
    <App />
  </ErrorBoundary>,
);
