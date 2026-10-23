import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import OverlayApp from "./OverlayApp";

const isOverlayWindow = new URLSearchParams(window.location.search).get("window") === "overlay";
if (isOverlayWindow) {
  document.documentElement.classList.add("overlay-document");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isOverlayWindow ? <OverlayApp /> : <App />}
  </React.StrictMode>,
);
