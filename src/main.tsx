import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initThemeFromStorage } from "./lib/theme";
import "./styles.css";

// 首屏同步套用主题（localStorage），避免 React 挂载后才切导致闪屏；后端 config 由 App 挂载后校正。
initThemeFromStorage();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
