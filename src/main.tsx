import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";

const savedTheme = localStorage.getItem("splits_theme_mode");
if (savedTheme === "dark" || savedTheme === "light") {
  document.documentElement.dataset.theme = savedTheme;
} else {
  delete document.documentElement.dataset.theme;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
