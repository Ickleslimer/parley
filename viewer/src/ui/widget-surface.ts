import type { ViewerApi } from "../ipc";

import { el } from "./dom";
import { LANDMARKS } from "./landmarks";
import { mountWidgetFeed } from "./widget-feed";

const PRESENTATION_HEARTBEAT_MS = 5_000;

export function mountWidgetSurface(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  root.className = "widget-surface-shell";
  root.removeAttribute("role");
  root.removeAttribute("aria-live");
  root.removeAttribute("aria-atomic");
  root.removeAttribute("tabindex");

  const frame = el("section", {
    id: LANDMARKS.widgetSurface,
    className: "widget-surface-frame",
  });
  frame.addEventListener("dragstart", (event) => {
    event.preventDefault();
  });
  frame.addEventListener("selectstart", (event) => {
    event.preventDefault();
  });
  root.replaceChildren(frame);

  let pointerReassert = Promise.resolve();
  let alive = true;
  let readySent = false;
  let activitySequence = 0;
  let domGeneration = 0;
  let lastHeartbeatMs = Number.NEGATIVE_INFINITY;

  const reassertOnPointerDown = (): void => {
    if (!alive) {
      return;
    }
    pointerReassert = api.widgetSurfacePointerDown().catch(() => undefined);
  };
  frame.addEventListener("pointerdown", reassertOnPointerDown);

  const reportActivity = (
    phase: "poll" | "dom-paint" | "animation-frame",
    changed: boolean,
  ): void => {
    activitySequence += 1;
    void api
      .reportWidgetSurfaceActivity({
        phase,
        sequence: activitySequence,
        generation: domGeneration,
        changed,
        documentVisibility: normalizedVisibility(),
        monotonicMs: monotonicMs(),
      })
      .catch(() => undefined);
  };

  const reportFrame = (): void => {
    if (typeof window.requestAnimationFrame !== "function") {
      return;
    }
    window.requestAnimationFrame(() => {
      if (alive) {
        reportActivity("animation-frame", true);
      }
    });
  };

  const feed = mountWidgetFeed({
    parent: frame,
    api,
    takePointerReassertion: () => pointerReassert,
    onPresented(changed) {
      if (!alive || !changed) {
        return;
      }
      domGeneration += 1;
      reportActivity("dom-paint", true);
      reportFrame();
    },
    onPolled(changed) {
      if (!alive) {
        return;
      }
      const now = monotonicMs();
      if (changed || now - lastHeartbeatMs >= PRESENTATION_HEARTBEAT_MS) {
        lastHeartbeatMs = now;
        reportActivity("poll", changed);
      }
    },
  });

  const signalReady = (): void => {
    if (readySent) {
      return;
    }
    readySent = true;
    void api.widgetSurfaceReady().catch(() => undefined);
  };
  signalReady();

  const stop = (): void => {
    if (!alive) {
      return;
    }
    alive = false;
    feed.stop();
    frame.removeEventListener("pointerdown", reassertOnPointerDown);
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function normalizedVisibility(): "visible" | "hidden" | "prerender" {
  const visibility = String(document.visibilityState);
  if (visibility === "hidden" || visibility === "prerender") {
    return visibility;
  }
  return "visible";
}

function monotonicMs(): number {
  return Math.max(0, Math.trunc(performance.now()));
}
