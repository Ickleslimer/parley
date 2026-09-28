import type { ViewerStatus, WidgetBrowserSnapshot, WidgetSnapshot } from "../contracts";
import type { ViewerApi } from "../ipc";

import type { WidgetSceneModel } from "./attribution";
import { el, setText } from "./dom";
import { loadErrorLabel } from "./labels";
import { LANDMARKS } from "./landmarks";
import { createSingleFlightPoller, WIDGET_POLL_MS } from "./poll";
import { widgetSceneModel } from "./status-model";
import { createPaperColumn } from "./widget-scene";

const MISSING_LABEL = "The selected exchange is no longer available";
const AMBIGUOUS_LABEL = "The selected exchange matches more than one record";
const LIVE_LABEL = "Following the newest exchange";
const EARLIER_LABEL = "Showing an earlier exchange";
const PRESENTATION_HEARTBEAT_MS = 5_000;

export function mountWidgetSurface(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  root.className = "widget-surface-shell";
  root.removeAttribute("role");
  root.removeAttribute("aria-live");
  root.removeAttribute("aria-atomic");
  root.removeAttribute("tabindex");

  const paper = createPaperColumn({
    liveId: LANDMARKS.widgetSurfaceLive,
    liveOwner: false,
  });
  const mode = el("p", { className: "widget-surface-mode" });
  mode.hidden = true;
  paper.live.prepend(mode);
  let pointerReassert = Promise.resolve();

  const older = mouseButton("Older", LANDMARKS.widgetSurfaceOlder, () => {
    browse(() => api.widgetBrowseOlder());
  });
  const newer = mouseButton("Newer", LANDMARKS.widgetSurfaceNewer, () => {
    browse(() => api.widgetBrowseNewer());
  });
  const live = mouseButton("Live", LANDMARKS.widgetSurfaceFollowLive, () => {
    browse(() => api.widgetBrowseLive());
  });
  const open = mouseButton("Open transcript", LANDMARKS.widgetSurfaceOpen, () => {
    if (open.disabled || !alive) {
      return;
    }
    const reassertion = pointerReassert;
    void reassertion.then(() => api.openWidgetExchange());
  });
  const controls = el("div", {
    id: LANDMARKS.widgetSurfaceControls,
    className: "widget-surface-controls",
    attrs: { role: "group", "aria-label": "Conversation position" },
    children: [older, newer, live, open],
  });
  paper.column.append(controls);

  const frame = el("section", {
    id: LANDMARKS.widgetSurface,
    className: "widget-surface-frame",
    children: [paper.column],
  });
  frame.addEventListener("dragstart", (event) => {
    event.preventDefault();
  });
  frame.addEventListener("selectstart", (event) => {
    event.preventDefault();
  });
  root.replaceChildren(frame);

  let status: ViewerStatus | null = null;
  let browser: WidgetBrowserSnapshot | null = null;
  let error: string | null = null;
  let alive = true;
  let epoch = 0;
  let browsing = false;
  let readySent = false;
  let activitySequence = 0;
  let domGeneration = 0;
  let lastPresentationRevision: string | null = null;
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

  const paint = (): boolean => {
    const selected = browser?.selectionState === "selected";
    const model = presentedModel(status, selected ? browser?.widget ?? null : blankWidget(), error, browser);
    const revision = presentationRevision(model, browser);
    const changed = revision !== lastPresentationRevision;
    paper.paint(model);
    paper.setLiveOwner(status?.desktopRuntimeState === "interactive");
    const modeText = browserMode(browser);
    setText(mode, modeText);
    mode.hidden = modeText.length === 0;
    older.disabled = browser?.hasOlder !== true;
    newer.disabled = browser?.hasNewer !== true;
    open.disabled = browser?.selectionState !== "selected";
    const following = browser?.followLive === true;
    live.setAttribute("aria-pressed", following ? "true" : "false");
    live.classList.toggle("is-latched", following);
    root.classList.toggle(
      "is-historical",
      browser?.selectionState === "selected" && browser.followLive === false,
    );
    root.dataset.selectionState = browser?.selectionState ?? "";
    root.dataset.followLive = following ? "true" : "false";
    if (changed) {
      lastPresentationRevision = revision;
      domGeneration += 1;
      reportActivity("dom-paint", true);
      reportFrame();
    }
    return changed;
  };

  const poller = createSingleFlightPoller(async () => {
    const token = epoch;
    try {
      const [nextStatus, nextBrowser] = await Promise.all([api.getStatus(), api.getWidgetBrowser()]);
      if (!alive || token !== epoch) {
        return;
      }
      status = nextStatus;
      browser = nextBrowser;
      error = null;
    } catch {
      if (!alive || token !== epoch) {
        return;
      }
      error = loadErrorLabel("load widget browser");
    }
    const changed = paint();
    const now = monotonicMs();
    if (changed || now - lastHeartbeatMs >= PRESENTATION_HEARTBEAT_MS) {
      lastHeartbeatMs = now;
      reportActivity("poll", changed);
    }
  }, WIDGET_POLL_MS);

  const browse = (action: () => Promise<WidgetBrowserSnapshot>): void => {
    if (!alive || browsing) {
      return;
    }
    browsing = true;
    epoch += 1;
    const token = epoch;
    const reassertion = pointerReassert;
    void Promise.resolve()
      .then(() => reassertion)
      .then(action)
      .then((next) => {
        if (!alive || token !== epoch) {
          return;
        }
        browser = next;
        error = null;
        paint();
      })
      .catch(() => {
        if (!alive || token !== epoch) {
          return;
        }
        error = loadErrorLabel("browse conversation");
        paint();
      })
      .finally(() => {
        if (token === epoch) {
          browsing = false;
        }
      });
  };

  const signalReady = (): void => {
    if (readySent) {
      return;
    }
    readySent = true;
    void api.widgetSurfaceReady().catch(() => undefined);
  };

  paint();
  signalReady();
  poller.start();

  const stop = (): void => {
    alive = false;
    epoch += 1;
    poller.stop();
    frame.removeEventListener("pointerdown", reassertOnPointerDown);
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function presentedModel(
  status: ViewerStatus | null,
  snapshot: WidgetSnapshot | null,
  loadError: string | null,
  browser: WidgetBrowserSnapshot | null,
): WidgetSceneModel {
  const model = widgetSceneModel({ status, snapshot, loadError });
  if (browser?.selectionState === "missing") {
    return { ...model, bubbles: [], idleLabel: MISSING_LABEL, liveRevision: "missing" };
  }
  if (browser?.selectionState === "ambiguous") {
    return { ...model, bubbles: [], idleLabel: AMBIGUOUS_LABEL, liveRevision: "ambiguous" };
  }
  return model;
}

function browserMode(browser: WidgetBrowserSnapshot | null): string {
  if (!browser || browser.selectionState !== "selected") {
    return "";
  }
  if (browser.followLive) {
    return LIVE_LABEL;
  }
  const newer = Number.isFinite(browser.newerCount) ? Math.max(0, Math.trunc(browser.newerCount)) : 0;
  if (newer === 1) {
    return `${EARLIER_LABEL}. 1 newer exchange.`;
  }
  if (newer > 1) {
    return `${EARLIER_LABEL}. ${newer} newer exchanges.`;
  }
  return EARLIER_LABEL;
}

function presentationRevision(
  model: WidgetSceneModel,
  browser: WidgetBrowserSnapshot | null,
): string {
  return [
    model.liveRevision,
    model.banner ?? "",
    model.idleLabel ?? "",
    model.loadError ?? "",
    model.sourceLabel,
    browser?.selectionState ?? "",
    browser?.followLive === true ? "live" : "historical",
    browser?.position ?? -1,
    browser?.total ?? 0,
    browser?.newerCount ?? 0,
  ].join("\u001d");
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

function blankWidget(): WidgetSnapshot {
  return {
    sessionKey: null,
    exchangeKey: null,
    sessionId: null,
    exchangeId: null,
    request: null,
    completion: null,
    pendingLabel: null,
  };
}

function mouseButton(label: string, id: string, onClick: () => void): HTMLButtonElement {
  const control = el("button", {
    id,
    className: "widget-surface-button",
    text: label,
    attrs: { type: "button", tabindex: "-1" },
  });
  control.tabIndex = -1;
  control.addEventListener("click", () => {
    if (control.disabled) {
      return;
    }
    onClick();
  });
  control.addEventListener("mousedown", (event) => {
    event.preventDefault();
  });
  control.addEventListener("keydown", (event) => {
    event.preventDefault();
    event.stopPropagation();
  });
  control.addEventListener("focus", () => {
    control.blur();
  });
  return control;
}
