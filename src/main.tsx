import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import QuickAdd from "./windows/QuickAdd";
import Settings from "./windows/Settings";
import "./styles.css";

// One bundle serves both windows; the window label picks the view.
const label = getCurrentWindow().label;
document.documentElement.dataset.window = label;

createRoot(document.getElementById("root")!).render(
  <StrictMode>{label === "quick-add" ? <QuickAdd /> : <Settings />}</StrictMode>,
);
