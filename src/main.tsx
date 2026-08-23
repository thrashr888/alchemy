import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { initTextScale } from "./lib/textScale";
import { installDiagnostics } from "./lib/diagnostics";
import { ErrorBoundary } from "./components/ErrorBoundary";

// Adopt the macOS Accessibility text size before first paint (shared by the
// main window and every pop-out — see lib/textScale.ts).
initTextScale();

// Catch errors before React mounts: a throw in the first render is the one
// most likely to leave a blank window (docs/RFC-diagnostics.md).
installDiagnostics();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
