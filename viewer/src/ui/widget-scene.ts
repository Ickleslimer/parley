import { el } from "./dom";
import { LANDMARKS } from "./landmarks";

export interface WidgetUnderlayHandle {
  column: HTMLDivElement;
}

export function createWidgetUnderlayScene(root: HTMLElement): WidgetUnderlayHandle {
  root.className = "widget-shell";
  root.removeAttribute("role");
  root.removeAttribute("aria-live");
  root.removeAttribute("aria-atomic");
  root.removeAttribute("tabindex");

  const column = el("div", {
    id: LANDMARKS.widgetColumn,
    className: "widget-underlay-host",
    attrs: { "aria-hidden": "true" },
  });
  const scene = el("section", {
    id: LANDMARKS.widgetScene,
    className: "widget-scene widget-underlay-scene",
    children: [column],
  });
  root.replaceChildren(scene);

  return { column };
}
