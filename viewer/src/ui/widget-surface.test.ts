// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";

import type { ViewerStatus, WidgetFeedPage } from "../contracts";
import { createSyntheticFixture } from "../fixtures/synthetic-api";
import type { ViewerApi } from "../ipc";

import { LANDMARKS } from "./landmarks";
import { mountWidget } from "./widget";
import { mountWidgetSurface } from "./widget-surface";

const HOSTILE = "<img src=x onerror=alert(1)><script>alert(1)</script>";

afterEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("widget surface", () => {
  it("renders the feed with mouse-only controls and no pager", async () => {
    const openWidgetEvent = vi.fn(async () => undefined);
    const openWidgetExchange = vi.fn(async () => undefined);
    const showDetail = vi.fn(async () => undefined);
    const widgetBrowseOlder = vi.fn(async () => {
      throw new Error("pager");
    });
    const reportWidgetSurfaceBounds = vi.fn(async () => watchingStatus());
    const retryInteractiveMode = vi.fn(async () => watchingStatus());
    const { root, stop } = mountSurface({
      page: feedPage(HOSTILE),
      openWidgetEvent,
      openWidgetExchange,
      showDetail,
      widgetBrowseOlder,
      reportWidgetSurfaceBounds,
      retryInteractiveMode,
    });
    await vi.waitFor(() => {
      expect(root.querySelector(".widget-feed-body")?.textContent).toBe(HOSTILE);
    });
    expect(root.querySelector("script")).toBeNull();
    expect(root.querySelector(`#${LANDMARKS.widgetSurface}`)).not.toBeNull();
    expect(root.querySelector(`#${LANDMARKS.widgetSurfaceOlder}`)).toBeNull();
    expect(root.querySelector(`#${LANDMARKS.widgetSurfaceNewer}`)).toBeNull();
    const status = root.querySelector(`#${LANDMARKS.widgetFeedStatus}`);
    const scroll = root.querySelector(`#${LANDMARKS.widgetFeedScroll}`);
    const controls = root.querySelector(`#${LANDMARKS.widgetSurfaceControls}`);
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(scroll?.contains(status)).toBe(false);
    expect(scroll?.contains(controls)).toBe(false);
    expect(root.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    const live = button(root, LANDMARKS.widgetFeedLiveToggle);
    expect(live.textContent).toBe("Following live");
    expect(live.getAttribute("aria-pressed")).toBe("true");
    expect(live.classList.contains("is-latched")).toBe(true);
    for (const id of [
      LANDMARKS.widgetFeedLoadEarlier,
      LANDMARKS.widgetFeedLiveToggle,
      LANDMARKS.widgetFeedOpenTranscript,
    ]) {
      const control = button(root, id);
      expect(control.tabIndex).toBe(-1);
      const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
      control.dispatchEvent(down);
      expect(down.defaultPrevented).toBe(true);
      control.focus();
      expect(document.activeElement).not.toBe(control);
      const key = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
      control.dispatchEvent(key);
      expect(key.defaultPrevented).toBe(true);
    }
    root.dispatchEvent(new WheelEvent("wheel", { deltaY: 80, bubbles: true }));
    expect(widgetBrowseOlder).not.toHaveBeenCalled();
    button(root, LANDMARKS.widgetFeedOpenTranscript).click();
    await vi.waitFor(() => {
      expect(openWidgetEvent).toHaveBeenCalledWith("response-1");
    });
    expect(openWidgetExchange).not.toHaveBeenCalled();
    expect(showDetail).not.toHaveBeenCalled();
    expect(reportWidgetSurfaceBounds).not.toHaveBeenCalled();
    expect(retryInteractiveMode).not.toHaveBeenCalled();
    stop();
  });

  it("reasserts the desktop band before a mouse feed action", async () => {
    let releaseReassertion: () => void = () => undefined;
    const widgetSurfacePointerDown = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          releaseReassertion = resolve;
        }),
    );
    const getWidgetFeed = vi.fn(async (before: string | null) =>
      before == null
        ? feedPage("Ship the engine", { hasEarlier: true, nextBefore: "exchange-1" })
        : feedPage("Earlier exact note", { hasEarlier: false }),
    );
    const { root, stop } = mountSurface({ getWidgetFeed, widgetSurfacePointerDown });
    await vi.waitFor(() => {
      expect(button(root, LANDMARKS.widgetFeedLoadEarlier).disabled).toBe(false);
    });
    const callsBefore = getWidgetFeed.mock.calls.length;
    const earlier = button(root, LANDMARKS.widgetFeedLoadEarlier);
    earlier.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    earlier.click();
    await Promise.resolve();
    expect(getWidgetFeed.mock.calls.length).toBe(callsBefore);
    expect(widgetSurfacePointerDown).toHaveBeenCalledTimes(1);
    releaseReassertion();
    await vi.waitFor(() => {
      expect(getWidgetFeed).toHaveBeenCalledWith("exchange-1");
    });
    stop();
  });

  it("signals readiness once and gives the surface the only polite region while interactive", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const currentStatus = watchingStatus();
    const widgetSurfaceReady = vi.fn(async () => currentStatus);
    const underlay = document.createElement("div");
    const surface = document.createElement("div");
    document.body.append(underlay, surface);
    const api = surfaceApi({
      status: currentStatus,
      page: feedPage("Ship the engine"),
      widgetSurfaceReady,
    });
    const underlayHandle = mountWidget(underlay, api);
    const surfaceHandle = mountWidgetSurface(surface, api);
    expect(widgetSurfaceReady).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(surface.querySelector(".widget-feed-body")?.textContent).toBe("Ship the engine");
    });
    expect(document.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    expect(surface.querySelector(`#${LANDMARKS.widgetFeedStatus}`)?.getAttribute("aria-live")).toBe("polite");
    expect(underlay.querySelector(`#${LANDMARKS.widgetColumn}`)?.getAttribute("aria-hidden")).toBe("true");
    expect(underlay.querySelector(`#${LANDMARKS.widgetCodex}`)?.getAttribute("aria-hidden")).toBe("true");
    expect(underlay.querySelector(".widget-live, .widget-slip, .widget-excerpt")).toBeNull();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(widgetSurfaceReady).toHaveBeenCalledTimes(1);
    underlayHandle.stop();
    surfaceHandle.stop();
  });

  it("reports only sanitized poll, DOM, and frame activity", async () => {
    const originalRequestAnimationFrame = window.requestAnimationFrame;
    window.requestAnimationFrame = (callback: FrameRequestCallback): number => {
      callback(performance.now());
      return 1;
    };
    const reportWidgetSurfaceActivity = vi.fn(
      async (_report: Parameters<ViewerApi["reportWidgetSurfaceActivity"]>[0]) => undefined,
    );
    const { stop } = mountSurface({
      page: feedPage("Ship the engine"),
      reportWidgetSurfaceActivity,
    });
    await vi.waitFor(() => {
      expect(reportWidgetSurfaceActivity).toHaveBeenCalledWith(
        expect.objectContaining({ phase: "dom-paint", generation: expect.any(Number) }),
      );
      expect(reportWidgetSurfaceActivity).toHaveBeenCalledWith(
        expect.objectContaining({ phase: "animation-frame", generation: expect.any(Number) }),
      );
      expect(reportWidgetSurfaceActivity).toHaveBeenCalledWith(
        expect.objectContaining({ phase: "poll", generation: expect.any(Number) }),
      );
    });
    for (const [report] of reportWidgetSurfaceActivity.mock.calls) {
      expect(Object.keys(report).sort()).toEqual([
        "changed",
        "documentVisibility",
        "generation",
        "monotonicMs",
        "phase",
        "sequence",
      ]);
    }
    stop();
    window.requestAnimationFrame = originalRequestAnimationFrame;
  });

  it("keeps passive fallback scene-only without restoring a second transcript", async () => {
    const fixture = createSyntheticFixture("passive-fallback");
    const underlay = document.createElement("div");
    const surface = document.createElement("div");
    document.body.append(underlay, surface);
    const underlayHandle = mountWidget(underlay, fixture.api);
    const surfaceHandle = mountWidgetSurface(surface, fixture.api);
    expect(underlay.querySelector(`#${LANDMARKS.widgetColumn}`)?.getAttribute("aria-hidden")).toBe("true");
    expect(underlay.querySelector(".widget-live, .widget-slip, .widget-excerpt")).toBeNull();
    expect(underlay.querySelectorAll('[aria-live="polite"]')).toHaveLength(0);
    expect(surface.querySelector('[aria-live="polite"]')).toBeNull();
    for (const image of underlay.querySelectorAll("img")) {
      image.dispatchEvent(new Event("error"));
    }
    expect(underlay.querySelectorAll("img.is-missing")).toHaveLength(3);
    expect(underlay.textContent).toBe("");
    underlayHandle.stop();
    surfaceHandle.stop();
  });

  it("keeps synthetic browser snapshots for historical, missing, and ambiguous fixtures", async () => {
    const historical = await createSyntheticFixture("historical").api.getWidgetBrowser();
    expect(historical.followLive).toBe(false);
    expect(historical.newerCount).toBe(2);
    const missing = await createSyntheticFixture("missing-selection").api.getWidgetBrowser();
    expect(missing.selectionState).toBe("missing");
    const ambiguous = await createSyntheticFixture("ambiguous").api.getWidgetBrowser();
    expect(ambiguous.selectionState).toBe("ambiguous");
    const passive = await createSyntheticFixture("passive-fallback").api.getStatus();
    expect(passive.desktopRuntimeState).toBe("passive-fallback");
    expect(passive.desktopFallbackReason).toBe("surface-z-order-invalid");
  });
});

function mountSurface(options: {
  page?: WidgetFeedPage;
  status?: ViewerStatus;
  getWidgetFeed?: ViewerApi["getWidgetFeed"];
  openWidgetEvent?: ViewerApi["openWidgetEvent"];
  openWidgetExchange?: ViewerApi["openWidgetExchange"];
  showDetail?: ViewerApi["showDetail"];
  widgetBrowseOlder?: ViewerApi["widgetBrowseOlder"];
  reportWidgetSurfaceBounds?: ViewerApi["reportWidgetSurfaceBounds"];
  reportWidgetSurfaceActivity?: ViewerApi["reportWidgetSurfaceActivity"];
  widgetSurfacePointerDown?: ViewerApi["widgetSurfacePointerDown"];
  retryInteractiveMode?: ViewerApi["retryInteractiveMode"];
  widgetSurfaceReady?: ViewerApi["widgetSurfaceReady"];
}): { root: HTMLDivElement; stop: () => void } {
  const root = document.createElement("div");
  document.body.append(root);
  const handle = mountWidgetSurface(
    root,
    surfaceApi({
      status: options.status ?? watchingStatus(),
      page: options.page ?? feedPage("Ship the engine"),
      getWidgetFeed: options.getWidgetFeed,
      openWidgetEvent: options.openWidgetEvent,
      openWidgetExchange: options.openWidgetExchange,
      showDetail: options.showDetail,
      widgetBrowseOlder: options.widgetBrowseOlder,
      reportWidgetSurfaceBounds: options.reportWidgetSurfaceBounds,
      reportWidgetSurfaceActivity: options.reportWidgetSurfaceActivity,
      widgetSurfacePointerDown: options.widgetSurfacePointerDown,
      retryInteractiveMode: options.retryInteractiveMode,
      widgetSurfaceReady: options.widgetSurfaceReady,
    }),
  );
  return { root, stop: handle.stop };
}

function surfaceApi(options: {
  status: ViewerStatus;
  page: WidgetFeedPage;
  getWidgetFeed?: ViewerApi["getWidgetFeed"];
  openWidgetEvent?: ViewerApi["openWidgetEvent"];
  openWidgetExchange?: ViewerApi["openWidgetExchange"];
  showDetail?: ViewerApi["showDetail"];
  widgetBrowseOlder?: ViewerApi["widgetBrowseOlder"];
  reportWidgetSurfaceBounds?: ViewerApi["reportWidgetSurfaceBounds"];
  reportWidgetSurfaceActivity?: ViewerApi["reportWidgetSurfaceActivity"];
  widgetSurfacePointerDown?: ViewerApi["widgetSurfacePointerDown"];
  retryInteractiveMode?: ViewerApi["retryInteractiveMode"];
  widgetSurfaceReady?: ViewerApi["widgetSurfaceReady"];
}): ViewerApi {
  return {
    getStatus: async () => options.status,
    getWidgetFeed: options.getWidgetFeed ?? (async () => options.page),
    getWidgetMessage: async () => null,
    openWidgetEvent: options.openWidgetEvent ?? (async () => undefined),
    widgetSurfacePointerDown: options.widgetSurfacePointerDown ?? (async () => undefined),
    widgetSurfaceReady: options.widgetSurfaceReady ?? (async () => options.status),
    reportWidgetSurfaceActivity: options.reportWidgetSurfaceActivity ?? (async () => undefined),
    reportWidgetSurfaceBounds: options.reportWidgetSurfaceBounds ?? (async () => options.status),
    retryInteractiveMode: options.retryInteractiveMode ?? (async () => options.status),
    showDetail: options.showDetail ?? (async () => undefined),
    openWidgetExchange: options.openWidgetExchange ?? (async () => undefined),
    widgetBrowseOlder: options.widgetBrowseOlder ?? (async () => {
      throw new Error("pager");
    }),
  } as unknown as ViewerApi;
}

function feedPage(
  body: string,
  options: { hasEarlier?: boolean; nextBefore?: string | null } = {},
): WidgetFeedPage {
  return {
    historyToken: "history-1",
    items: [
      {
        exchangeKey: "exchange-1",
        sessionKey: "session-a",
        timestampMs: 1,
        request: {
          eventKey: "request-1",
          eventType: "request",
          speaker: "codex",
          recipient: "grok",
          timestampMs: 1,
          status: "ok",
          body,
          fullCharacterLength: Array.from(body).length,
          truncated: false,
          projection: "exact",
          contextOmitted: false,
        },
        completion: {
          eventKey: "response-1",
          eventType: "response",
          speaker: "grok",
          recipient: "codex",
          timestampMs: 2,
          status: "ok",
          body: "done & exact",
          fullCharacterLength: 12,
          truncated: false,
          projection: "exact",
          contextOmitted: false,
        },
        pendingLabel: null,
      },
    ],
    nextBeforeExchangeKey: options.nextBefore ?? null,
    hasEarlier: options.hasEarlier === true,
    totalExchanges: 1,
    totalEvents: 2,
    resetRequired: false,
  };
}

function watchingStatus(): ViewerStatus {
  return {
    sourceState: "watching",
    generation: 1,
    bytesRead: 100,
    sessionCount: 1,
    exchangeCount: 1,
    lastEventTimestampMs: 0,
    trayAvailable: true,
    underlayState: "attached",
    desktopRuntimeState: "interactive",
    desktopFallbackReason: null,
    widgetVisible: true,
    diagnostics: {
      malformedLines: 0,
      oversizedLines: 0,
      unsupportedRecords: 0,
      duplicateEvents: 0,
      ioErrors: 0,
      aliasCollisions: 0,
      lastError: null,
    },
    sources: [],
  };
}

function button(root: ParentNode, id: string): HTMLButtonElement {
  const node = root.querySelector<HTMLButtonElement>(`#${id}`);
  if (!node) {
    throw new Error(`Missing ${id}`);
  }
  return node;
}
