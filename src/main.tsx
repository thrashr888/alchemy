import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { initTextScale } from "./lib/textScale";

// Adopt the macOS Accessibility text size before first paint (shared by the
// main window and every pop-out — see lib/textScale.ts).
initTextScale();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
