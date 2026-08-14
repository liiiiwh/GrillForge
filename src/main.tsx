import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import QuickPanel from "./QuickPanel";

const isQuickPanel = new URLSearchParams(window.location.search).get("window") === "quick-menu";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isQuickPanel ? <QuickPanel /> : <App />}
  </React.StrictMode>,
);
