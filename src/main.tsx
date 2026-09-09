import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";

// v2.0.0 — wrap the App tree in an ErrorBoundary so any top-level
// render error displays on screen instead of producing a blank
// black window. Built during the v2.0 black-screen investigation;
// the boundary itself is not a fix for the underlying crash (if
// any), but it makes the error visible and copyable so the user
// can paste the stack to the developer.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
