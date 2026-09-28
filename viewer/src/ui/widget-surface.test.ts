// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  MessagePreview,
  ViewerStatus,
  WidgetBrowserSnapshot,
  WidgetSnapshot,
} from "../contracts";
import { createSyntheticFixture } from "../fixtures/synthetic-api";
import type { ViewerApi } from "../ipc";

import { LANDMARKS } from "./landmarks";
import { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";
import { mountWidget } from "./widget";
import { mountWidgetSurface } from "./widget-surface";

const HOSTILE = "<img src=x onerror=alert(1)><script>alert(1)</script>";

const preview = (overrides: Partial<MessagePreview> = {}): MessagePreview => ({
  eventKey: "key-req-1",
  eventId: "req-1",
  eventType: "request",
  speaker: "codex",
  recipient: "grok",
  timestampMs: 0,
  status: "started",
  excerpt: "Ship the engine",
  excerptExtracted: true,
  contentLength: 16,
  ...overrides,
});

const status = (overrides: Partial<ViewerStatus> = {}): ViewerStatus => ({
  sourceState: "watching",
  generation: 1,
  bytesRead: 100,
  sessionCount: 1,
  exchangeCount: 4,
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
  ...overrides,
});

const widget = (overrides: Partial<WidgetSnapshot> = {}): WidgetSnapshot => ({
  sessionKey: "key-s-1",
  exchangeKey: "key-ex-1",
  sessionId: "s-1",
  exchangeId: "ex-1",
  request: preview(),
  completion: preview({
    eventKey: "key-res-1",
    eventId: "res-1",
    eventType: "response",
    speaker: "grok",
    recipient: "codex",
    excerpt: "done & exact",
    excerptExtracted: false,
    status: "completed",
  }),
  pendingLabel: null,
  ...overrides,
});

const browser = (overrides: Partial<WidgetBrowserSnapshot> = {}): WidgetBrowserSnapshot => ({
  followLive: true,
  selectionState: "selected",
  position: 0,
  total: 4,
  hasOlder: true,
  hasNewer: false,
  newerCount: 0,
  widget: widget(),
  ...overrides,
});

function slips(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(".widget-slip"));
}

function button(root: ParentNode, id: string): HTMLButtonElement {
  const node = root.querySelector<HTMLButtonElement>(`#${id}`);
  if (!node) {
    throw new Error(`Missing ${id}`);
  }
  return node;
}

afterEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("widget surface", () => {
  it("renders exact text, attribution, and mouse-only controls without keyboard focus", async () => {
    const current = browser({
      widget: widget({ request: preview({ excerpt: HOSTILE }) }),
    });
    const openWidgetExchange = vi.fn(async () => undefined);
    const showDetail = vi.fn(async () => undefined);
    const reportWidgetSurfaceBounds = vi.fn(async () => status());
    const retryInteractiveMode = vi.fn(async () => status());
    const widgetBrowseOlder = vi.fn(async () => current);
    const { root, handle } = mountSurface({
      browser: current,
      openWidgetExchange,
      showDetail,
      reportWidgetSurfaceBounds,
      retryInteractiveMode,
      widgetBrowseOlder,
    });
    await vi.waitFor(() => {
      expect(root.querySelector(".widget-excerpt")?.textContent).toBe(HOSTILE);
    });
    const live = root.querySelector(`#${LANDMARKS.widgetSurfaceLive}`);
    const controls = root.querySelector(`#${LANDMARKS.widgetSurfaceControls}`);
    expect(root.querySelector(`#${LANDMARKS.widgetSurface}`)).not.toBeNull();
    expect(root.querySelector("img, script")).toBeNull();
    expect(live?.querySelector("img, script")).toBeNull();
    expect(live?.contains(controls)).toBe(false);
    expect(live?.getAttribute("aria-live")).toBe("polite");
    expect(root.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    expect(slips(root).map((slip) => slip.dataset.tail)).toEqual(["toward-codex", "toward-grok"]);
    expect(slips(root)[0]?.textContent).toContain(EXTRACTED_TASK_LABEL);
    expect(slips(root)[1]?.textContent).toContain("done & exact");
    expect(root.textContent).toContain("Following the newest exchange");
    const liveButton = button(root, LANDMARKS.widgetSurfaceFollowLive);
    expect(liveButton.getAttribute("aria-pressed")).toBe("true");
    expect(liveButton.classList.contains("is-latched")).toBe(true);
    for (const id of [
      LANDMARKS.widgetSurfaceOlder,
      LANDMARKS.widgetSurfaceNewer,
      LANDMARKS.widgetSurfaceFollowLive,
      LANDMARKS.widgetSurfaceOpen,
    ]) {
      const control = button(root, id);
      expect(control.tabIndex).toBe(-1);
      expect(control.hidden).toBe(false);
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
    button(root, LANDMARKS.widgetSurfaceOpen).click();
    await vi.waitFor(() => {
      expect(openWidgetExchange).toHaveBeenCalledTimes(1);
    });
    expect(showDetail).not.toHaveBeenCalled();
    expect(reportWidgetSurfaceBounds).not.toHaveBeenCalled();
    expect(retryInteractiveMode).not.toHaveBeenCalled();
    expect(root.querySelector("[tabindex='0']")).toBeNull();
    handle.stop();
  });

  it("reasserts the desktop band on pointerdown before browsing", async () => {
    let releaseReassertion: () => void = () => undefined;
    const widgetSurfacePointerDown = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          releaseReassertion = resolve;
        }),
    );
    const widgetBrowseOlder = vi.fn(async () => browser({ followLive: false, position: 1 }));
    const mounted = mountSurface({
      browser: browser(),
      widgetSurfacePointerDown,
      widgetBrowseOlder,
    });
    await vi.waitFor(() => {
      expect(button(mounted.root, LANDMARKS.widgetSurfaceOlder).disabled).toBe(false);
    });

    const older = button(mounted.root, LANDMARKS.widgetSurfaceOlder);
    older.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    older.click();

    expect(widgetSurfacePointerDown).toHaveBeenCalledTimes(1);
    await Promise.resolve();
    expect(widgetBrowseOlder).not.toHaveBeenCalled();
    releaseReassertion();
    await vi.waitFor(() => {
      expect(widgetBrowseOlder).toHaveBeenCalledTimes(1);
    });
    mounted.handle.stop();
  });

  it("shows pending, error, missing, ambiguous, and empty states without selecting a hidden payload", async () => {
    const pendingBrowser = browser({
      hasOlder: false,
      widget: widget({
        completion: null,
        pendingLabel: PENDING_LABEL,
      }),
    });
    const pending = mountSurface({ browser: pendingBrowser });
    await vi.waitFor(() => {
      expect(slips(pending.root)[1]?.dataset.kind).toBe("pending");
    });
    expect(slips(pending.root)[1]?.dataset.tail).toBe("toward-grok");
    expect(slips(pending.root)[1]?.textContent).toContain(PENDING_LABEL);
    expect(button(pending.root, LANDMARKS.widgetSurfaceOlder).disabled).toBe(true);
    pending.handle.stop();

    const errorBrowser = browser({
      widget: widget({
        completion: preview({
          eventKey: "key-err-1",
          eventId: "err-1",
          eventType: "error",
          speaker: "codex",
          excerpt: "boom",
          excerptExtracted: false,
          status: "failed",
        }),
      }),
    });
    const failed = mountSurface({ browser: errorBrowser });
    await vi.waitFor(() => {
      expect(slips(failed.root)[1]?.dataset.kind).toBe("parley-error");
    });
    expect(slips(failed.root)[1]?.dataset.tail).toBe("none");
    expect(slips(failed.root)[1]?.textContent).toContain(PARLEY_ERROR_LABEL);
    expect(slips(failed.root)[1]?.textContent).toContain("boom");
    failed.handle.stop();

    const unknown = mountSurface({
      browser: browser({
        widget: widget({
          request: preview({ speaker: "Nova", recipient: "codex", excerpt: "from nova" }),
          completion: null,
          pendingLabel: null,
        }),
      }),
    });
    await vi.waitFor(() => {
      expect(slips(unknown.root)[0]?.dataset.speaker).toBe("unknown");
    });
    expect(slips(unknown.root)[0]?.dataset.tail).toBe("none");
    expect(slips(unknown.root)[0]?.textContent).toContain("Nova");
    expect(slips(unknown.root)[0]?.textContent).toContain("from nova");
    unknown.handle.stop();

    const hidden = widget({ request: preview({ excerpt: "must stay hidden" }) });
    const missing = mountSurface({
      browser: browser({
        followLive: false,
        selectionState: "missing",
        hasOlder: true,
        hasNewer: true,
        widget: hidden,
      }),
    });
    await vi.waitFor(() => {
      expect(missing.root.textContent).toContain("The selected exchange is no longer available");
    });
    expect(missing.root.textContent).not.toContain("must stay hidden");
    expect(slips(missing.root)).toHaveLength(0);
    expect(button(missing.root, LANDMARKS.widgetSurfaceOpen).disabled).toBe(true);
    button(missing.root, LANDMARKS.widgetSurfaceOpen).click();
    expect(missing.openWidgetExchange).not.toHaveBeenCalled();
    missing.handle.stop();

    const ambiguous = mountSurface({
      browser: browser({
        selectionState: "ambiguous",
        widget: hidden,
      }),
    });
    await vi.waitFor(() => {
      expect(ambiguous.root.textContent).toContain("The selected exchange matches more than one record");
    });
    expect(slips(ambiguous.root)).toHaveLength(0);
    expect(button(ambiguous.root, LANDMARKS.widgetSurfaceOpen).disabled).toBe(true);
    ambiguous.handle.stop();

    const empty = mountSurface({
      status: status({ sourceState: "none", sessionCount: 0, exchangeCount: 0 }),
      browser: browser({
        selectionState: "empty",
        total: 0,
        hasOlder: false,
        hasNewer: false,
        widget: {
          sessionKey: null,
          exchangeKey: null,
          sessionId: null,
          exchangeId: null,
          request: null,
          completion: null,
          pendingLabel: null,
        },
      }),
    });
    await vi.waitFor(() => {
      expect(empty.root.querySelector(".widget-idle")?.textContent).toBe("No event log selected");
    });
    expect(slips(empty.root)).toHaveLength(0);
    expect(button(empty.root, LANDMARKS.widgetSurfaceNewer).disabled).toBe(true);
    expect(button(empty.root, LANDMARKS.widgetSurfaceFollowLive).getAttribute("aria-pressed")).toBe(
      "true",
    );
    empty.handle.stop();
  });

  it("leaves live for an earlier exchange and returns only through the live control", async () => {
    const historical = browser({
      followLive: false,
      position: 2,
      hasOlder: true,
      hasNewer: true,
      newerCount: 2,
      widget: widget({
        request: preview({ excerpt: "Earlier exact note" }),
      }),
    });
    const liveAgain = browser({ widget: widget({ request: preview({ excerpt: "Newest exact note" }) }) });
    const widgetBrowseOlder = vi.fn(async () => historical);
    const widgetBrowseLive = vi.fn(async () => liveAgain);
    const mounted = mountSurface({
      browser: browser(),
      widgetBrowseOlder,
      widgetBrowseLive,
    });
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Ship the engine");
    });
    button(mounted.root, LANDMARKS.widgetSurfaceOlder).click();
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Earlier exact note");
    });
    expect(widgetBrowseOlder).toHaveBeenCalledTimes(1);
    expect(mounted.root.classList.contains("is-historical")).toBe(true);
    expect(mounted.root.textContent).toContain("Showing an earlier exchange. 2 newer exchanges.");
    expect(button(mounted.root, LANDMARKS.widgetSurfaceFollowLive).getAttribute("aria-pressed")).toBe(
      "false",
    );
    button(mounted.root, LANDMARKS.widgetSurfaceFollowLive).click();
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Newest exact note");
    });
    expect(widgetBrowseLive).toHaveBeenCalledTimes(1);
    expect(mounted.root.classList.contains("is-historical")).toBe(false);
    expect(button(mounted.root, LANDMARKS.widgetSurfaceFollowLive).classList.contains("is-latched")).toBe(
      true,
    );
    mounted.handle.stop();
  });

  it("does not rewrite the live region on an identical poll", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const current = browser();
    const getWidgetBrowser = vi.fn(async () => current);
    const mounted = mountSurface({ browser: current, getWidgetBrowser });
    await vi.waitFor(() => {
      expect(mounted.root.querySelector(".widget-excerpt")?.textContent).toBe("Ship the engine");
    });
    const excerpt = mounted.root.querySelector(".widget-excerpt");
    const live = mounted.root.querySelector(`#${LANDMARKS.widgetSurfaceLive}`);
    const liveText = live?.textContent;
    current.widget = widget({
      ...current.widget,
    });
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(getWidgetBrowser.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
    expect(mounted.root.querySelector(".widget-excerpt")).toBe(excerpt);
    expect(live?.textContent).toBe(liveText);
    mounted.handle.stop();
  });

  it("signals readiness once and gives the surface the only polite region while interactive", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const currentStatus = status();
    const currentBrowser = browser();
    const widgetSurfaceReady = vi.fn(async () => currentStatus);
    const underlay = document.createElement("div");
    const surface = document.createElement("div");
    document.body.append(underlay, surface);
    const api = surfaceApi({
      status: currentStatus,
      browser: currentBrowser,
      widgetSurfaceReady,
    });
    const underlayHandle = mountWidget(underlay, api);
    const surfaceHandle = mountWidgetSurface(surface, api);
    expect(widgetSurfaceReady).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(surface.querySelector(".widget-excerpt")?.textContent).toBe("Ship the engine");
    });
    expect(document.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    expect(surface.querySelector(`#${LANDMARKS.widgetSurfaceLive}`)?.getAttribute("aria-live")).toBe(
      "polite",
    );
    expect(underlay.querySelector(`#${LANDMARKS.widgetColumn}`)?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    expect(underlay.querySelector(`#${LANDMARKS.widgetCodex}`)?.getAttribute("aria-hidden")).toBe(
      "true",
    );
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
    const mounted = mountSurface({
      browser: browser(),
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
    mounted.handle.stop();
    window.requestAnimationFrame = originalRequestAnimationFrame;
  });

  it("keeps passive fallback scene-only without restoring a second transcript", async () => {
    const fixture = createSyntheticFixture("passive-fallback");
    const underlay = document.createElement("div");
    const surface = document.createElement("div");
    document.body.append(underlay, surface);
    const underlayHandle = mountWidget(underlay, fixture.api);
    const surfaceHandle = mountWidgetSurface(surface, fixture.api);
    expect(underlay.querySelector(`#${LANDMARKS.widgetColumn}`)?.getAttribute("aria-hidden")).toBe(
      "true",
    );
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

  it("exposes historical, missing, and ambiguous synthetic fixtures", async () => {
    const historical = createSyntheticFixture("historical");
    const historicalBrowser = await historical.api.getWidgetBrowser();
    expect(historicalBrowser.followLive).toBe(false);
    expect(historicalBrowser.newerCount).toBe(2);
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
  status?: ViewerStatus;
  browser: WidgetBrowserSnapshot;
  getWidgetBrowser?: ViewerApi["getWidgetBrowser"];
  widgetBrowseOlder?: ViewerApi["widgetBrowseOlder"];
  widgetBrowseNewer?: ViewerApi["widgetBrowseNewer"];
  widgetBrowseLive?: ViewerApi["widgetBrowseLive"];
  openWidgetExchange?: ViewerApi["openWidgetExchange"];
  showDetail?: ViewerApi["showDetail"];
  reportWidgetSurfaceBounds?: ViewerApi["reportWidgetSurfaceBounds"];
  reportWidgetSurfaceActivity?: ViewerApi["reportWidgetSurfaceActivity"];
  widgetSurfacePointerDown?: ViewerApi["widgetSurfacePointerDown"];
  retryInteractiveMode?: ViewerApi["retryInteractiveMode"];
  widgetSurfaceReady?: ViewerApi["widgetSurfaceReady"];
}): { root: HTMLDivElement; handle: { stop: () => void }; openWidgetExchange: ReturnType<typeof vi.fn> } {
  const root = document.createElement("div");
  document.body.append(root);
  const openWidgetExchange = vi.fn(options.openWidgetExchange ?? (async () => undefined));
  const handle = mountWidgetSurface(
    root,
    surfaceApi({
      status: options.status ?? status(),
      browser: options.browser,
      getWidgetBrowser: options.getWidgetBrowser,
      widgetBrowseOlder: options.widgetBrowseOlder,
      widgetBrowseNewer: options.widgetBrowseNewer,
      widgetBrowseLive: options.widgetBrowseLive,
      openWidgetExchange,
      showDetail: options.showDetail,
      reportWidgetSurfaceBounds: options.reportWidgetSurfaceBounds,
      reportWidgetSurfaceActivity: options.reportWidgetSurfaceActivity,
      widgetSurfacePointerDown: options.widgetSurfacePointerDown,
      retryInteractiveMode: options.retryInteractiveMode,
      widgetSurfaceReady: options.widgetSurfaceReady,
    }),
  );
  return { root, handle, openWidgetExchange };
}

function surfaceApi(options: {
  status: ViewerStatus;
  browser: WidgetBrowserSnapshot;
  getWidgetBrowser?: ViewerApi["getWidgetBrowser"];
  widgetBrowseOlder?: ViewerApi["widgetBrowseOlder"];
  widgetBrowseNewer?: ViewerApi["widgetBrowseNewer"];
  widgetBrowseLive?: ViewerApi["widgetBrowseLive"];
  openWidgetExchange?: ViewerApi["openWidgetExchange"];
  showDetail?: ViewerApi["showDetail"];
  reportWidgetSurfaceBounds?: ViewerApi["reportWidgetSurfaceBounds"];
  reportWidgetSurfaceActivity?: ViewerApi["reportWidgetSurfaceActivity"];
  widgetSurfacePointerDown?: ViewerApi["widgetSurfacePointerDown"];
  retryInteractiveMode?: ViewerApi["retryInteractiveMode"];
  widgetSurfaceReady?: ViewerApi["widgetSurfaceReady"];
}): ViewerApi {
  const currentStatus = options.status;
  const currentBrowser = options.browser;
  return {
    getStatus: async () => currentStatus,
    getWidgetSnapshot: async () => currentBrowser.widget,
    getWidgetBrowser: options.getWidgetBrowser ?? (async () => currentBrowser),
    widgetBrowseOlder: options.widgetBrowseOlder ?? (async () => currentBrowser),
    widgetBrowseNewer: options.widgetBrowseNewer ?? (async () => currentBrowser),
    widgetBrowseLive: options.widgetBrowseLive ?? (async () => currentBrowser),
    openWidgetExchange: options.openWidgetExchange ?? (async () => undefined),
    reportWidgetSurfaceBounds: options.reportWidgetSurfaceBounds ?? (async () => currentStatus),
    reportWidgetSurfaceActivity:
      options.reportWidgetSurfaceActivity ?? (async () => undefined),
    widgetSurfacePointerDown: options.widgetSurfacePointerDown ?? (async () => undefined),
    widgetSurfaceReady: options.widgetSurfaceReady ?? (async () => currentStatus),
    retryInteractiveMode: options.retryInteractiveMode ?? (async () => currentStatus),
    showDetail: options.showDetail ?? (async () => undefined),
  } as unknown as ViewerApi;
}
