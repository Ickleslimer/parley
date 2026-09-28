// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";

import type { ViewerStatus, WidgetSurfaceBoundsReport } from "../contracts";
import type { ViewerApi } from "../ipc";

import { LANDMARKS } from "./landmarks";
import { mountWidget, validatedSurfaceBounds } from "./widget";
import { createWidgetUnderlayScene } from "./widget-scene";

afterEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("neutral widget underlay", () => {
  it("creates one measurable accessibility-hidden geometry host", () => {
    const root = document.createElement("div");
    root.setAttribute("role", "status");
    root.setAttribute("aria-live", "polite");
    root.tabIndex = 0;
    const scene = createWidgetUnderlayScene(root);

    vi.spyOn(scene.column, "getBoundingClientRect").mockReturnValue(new DOMRect(8, 8, 704, 544));
    expect(scene.column.getBoundingClientRect()).toEqual(new DOMRect(8, 8, 704, 544));
    expect(scene.column.id).toBe(LANDMARKS.widgetColumn);
    expect(scene.column.getAttribute("aria-hidden")).toBe("true");
    expect(scene.column.childElementCount).toBe(0);
    expect(root.querySelector(`#${LANDMARKS.widgetScene}`)).not.toBeNull();
    expect(root.querySelector("img, button, [aria-live], .widget-feed-row, .widget-feed-body")).toBeNull();
    expect(root.textContent).toBe("");
    expect(root.hasAttribute("role")).toBe(false);
    expect(root.hasAttribute("tabindex")).toBe(false);
  });

  it("stays transcript-free across every desktop runtime state", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const current = status();
    const root = document.createElement("div");
    document.body.append(root);
    const handle = mountWidget(root, {
      getStatus: async () => current,
      reportWidgetSurfaceBounds: async () => current,
    } as unknown as ViewerApi);

    for (const desktopRuntimeState of [
      "interactive",
      "interactive-starting",
      "passive-fallback",
      "passive",
    ] as const) {
      current.desktopRuntimeState = desktopRuntimeState;
      await vi.advanceTimersByTimeAsync(500);
      expect(root.querySelector(`#${LANDMARKS.widgetColumn}`)?.getAttribute("aria-hidden")).toBe("true");
      expect(root.querySelector("img, button, [aria-live], .widget-feed-row, .widget-feed-body")).toBeNull();
      expect(root.textContent).toBe("");
    }
    handle.stop();
  });
});

describe("underlay geometry handshake", () => {
  it("reports quantized in-viewport bounds only when geometry changes", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    setViewport(720, 560, 2);
    const box = { left: 8.2, top: 8.4, width: 703.8, height: 543.8 };
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.id === LANDMARKS.widgetColumn
        ? new DOMRect(box.left, box.top, box.width, box.height)
        : new DOMRect();
    });
    const current = status({ desktopRuntimeState: "passive" });
    const reportWidgetSurfaceBounds = vi.fn(async (_report: WidgetSurfaceBoundsReport) => current);
    const root = document.createElement("div");
    document.body.append(root);
    const handle = mountWidget(root, {
      getStatus: async () => current,
      reportWidgetSurfaceBounds,
    } as unknown as ViewerApi);

    await vi.waitFor(() => expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(1));
    expect(reportWidgetSurfaceBounds.mock.calls[0]?.[0]).toEqual({
      left: 8,
      top: 8.5,
      width: 704,
      height: 544,
      viewportWidth: 720,
      viewportHeight: 560,
      devicePixelRatio: 2,
    });
    await vi.advanceTimersByTimeAsync(500);
    expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(1);

    box.width = 680.2;
    window.dispatchEvent(new Event("resize"));
    await vi.waitFor(() => expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(2));
    expect(reportWidgetSurfaceBounds.mock.calls[1]?.[0]).toMatchObject({ width: 680 });
    handle.stop();
  });

  it("observes host layout changes without a window resize", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    setViewport(720, 560, 1);
    const box = { left: 8, top: 8, width: 704, height: 544 };
    const callbacks: ResizeObserverCallback[] = [];
    const disconnect = vi.fn();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: ResizeObserverCallback) {
          callbacks.push(callback);
        }
        observe(): void {}
        unobserve(): void {}
        disconnect = disconnect;
      },
    );
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.id === LANDMARKS.widgetColumn
        ? new DOMRect(box.left, box.top, box.width, box.height)
        : new DOMRect();
    });
    const current = status({ desktopRuntimeState: "passive" });
    const reportWidgetSurfaceBounds = vi.fn(async (_report: WidgetSurfaceBoundsReport) => current);
    const root = document.createElement("div");
    document.body.append(root);
    const handle = mountWidget(root, {
      getStatus: async () => current,
      reportWidgetSurfaceBounds,
    } as unknown as ViewerApi);

    await vi.waitFor(() => expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(1));
    box.width = 680;
    callbacks[0]?.([], {} as ResizeObserver);
    await vi.waitFor(() => expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(2));
    expect(reportWidgetSurfaceBounds.mock.calls[1]?.[0]).toMatchObject({ width: 680 });
    handle.stop();
    expect(disconnect).toHaveBeenCalledTimes(1);
  });

  it("bounds retries per signature and resets them after fallback recovery", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    setViewport(720, 560, 1);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.id === LANDMARKS.widgetColumn ? new DOMRect(8, 8, 704, 544) : new DOMRect();
    });
    const current = status({
      desktopRuntimeState: "passive-fallback",
      desktopFallbackReason: "surface-z-order-invalid",
    });
    const reportWidgetSurfaceBounds = vi.fn(async (_report: WidgetSurfaceBoundsReport) => {
      throw new Error("rejected");
    });
    const root = document.createElement("div");
    document.body.append(root);
    const handle = mountWidget(root, {
      getStatus: async () => current,
      reportWidgetSurfaceBounds,
    } as unknown as ViewerApi);

    await vi.advanceTimersByTimeAsync(2_000);
    expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(3);
    current.desktopRuntimeState = "interactive-starting";
    current.desktopFallbackReason = null;
    await vi.advanceTimersByTimeAsync(500);
    expect(reportWidgetSurfaceBounds).toHaveBeenCalledTimes(4);
    handle.stop();
  });
});

describe("validated surface bounds", () => {
  it("quantizes to physical pixels and rejects invalid geometry", () => {
    expect(validatedSurfaceBounds({ left: 10.2, top: 4.4, width: 100.2, height: 50.2 }, 720, 560, 2)).toEqual({
      left: 10,
      top: 4.5,
      width: 100,
      height: 50,
      viewportWidth: 720,
      viewportHeight: 560,
      devicePixelRatio: 2,
    });
    expect(validatedSurfaceBounds({ left: 700, top: 0, width: 100, height: 40 }, 720, 560, 1)).toBeNull();
    expect(validatedSurfaceBounds({ left: -2, top: 0, width: 40, height: 40 }, 720, 560, 1)).toBeNull();
    expect(validatedSurfaceBounds({ left: 0, top: 0, width: 0, height: 40 }, 720, 560, 1)).toBeNull();
    expect(validatedSurfaceBounds({ left: 0, top: 0, width: 40, height: 40 }, 720, 560, 0)).toBeNull();
    expect(
      validatedSurfaceBounds({ left: Number.NaN, top: 0, width: 40, height: 40 }, 720, 560, 1),
    ).toBeNull();
  });
});

function setViewport(width: number, height: number, ratio: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  Object.defineProperty(window, "innerHeight", { configurable: true, value: height });
  Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: ratio });
}

function status(overrides: Partial<ViewerStatus> = {}): ViewerStatus {
  return {
    sourceState: "watching",
    generation: 1,
    bytesRead: 0,
    sessionCount: 0,
    exchangeCount: 0,
    lastEventTimestampMs: null,
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
  };
}
