import "./styles.css";

import { viewerApi } from "./ipc";
import { mountDetail } from "./ui/detail";
import { mountWidget } from "./ui/widget";
import { mountWidgetSurface } from "./ui/widget-surface";

const app = document.querySelector<HTMLElement>("#app");

if (!app) {
  throw new Error("Missing application root");
}

const view = new URLSearchParams(window.location.search).get("view");
const fixture = new URLSearchParams(window.location.search).get("fixture");
const development =
  (import.meta as ImportMeta & { env?: { DEV?: boolean } }).env?.DEV === true;

const surface =
  view === "widget" ? "widget" : view === "widget-surface" ? "widget-surface" : "detail";

if (development && fixture) {
  void import("./fixtures/mount").then(({ mountSyntheticFixture }) => {
    mountSyntheticFixture(app, viewerApi, surface, fixture);
  });
} else if (view === "widget-surface") {
  document.title = "Parley";
  mountWidgetSurface(app, viewerApi);
} else if (view === "widget") {
  document.title = "Parley";
  mountWidget(app, viewerApi);
} else {
  document.title = "Parley Conversation Viewer";
  mountDetail(app, viewerApi);
}
