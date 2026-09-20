import "./styles.css";

import { viewerApi } from "./ipc";
import { mountDetail } from "./ui/detail";
import { mountWidget } from "./ui/widget";

const app = document.querySelector<HTMLElement>("#app");

if (!app) {
  throw new Error("Missing application root");
}

const view = new URLSearchParams(window.location.search).get("view");

if (view === "widget") {
  document.title = "Parley";
  mountWidget(app, viewerApi);
} else {
  document.title = "Parley Conversation Viewer";
  mountDetail(app, viewerApi);
}
