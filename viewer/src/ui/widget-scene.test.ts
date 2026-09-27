// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import type { MessagePreview, ViewerStatus, WidgetSnapshot } from "../contracts";
import type { ViewerApi } from "../ipc";

import { LANDMARKS } from "./landmarks";
import { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";
import { widgetSceneModel } from "./status-model";
import { mountWidget } from "./widget";
import { createWidgetScene } from "./widget-scene";

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
  ...overrides,
});

const snapshot = (overrides: Partial<WidgetSnapshot> = {}): WidgetSnapshot => ({
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

function paint(modelSnapshot: WidgetSnapshot, modelStatus: ViewerStatus | null = status()) {
  const root = document.createElement("div");
  document.body.append(root);
  const scene = createWidgetScene(root);
  scene.paint(
    widgetSceneModel({
      status: modelStatus,
      snapshot: modelSnapshot,
      loadError: null,
      utc: true,
    }),
  );
  return { root, scene };
}

function slips(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(".widget-slip"));
}

afterEach(() => {
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("widget scene", () => {
  it("keeps figures outside one polite live region and exposes no controls", () => {
    const { root } = paint(snapshot({ request: preview({ excerpt: HOSTILE }) }));
    const scene = root.querySelector(`#${LANDMARKS.widgetScene}`);
    const live = root.querySelector(`#${LANDMARKS.widgetLive}`);
    const codex = root.querySelector(`#${LANDMARKS.widgetCodex}`);
    const grok = root.querySelector(`#${LANDMARKS.widgetGrok}`);
    expect(scene).not.toBeNull();
    expect(live?.getAttribute("aria-live")).toBe("polite");
    expect(live?.getAttribute("role")).toBe("status");
    expect(live?.contains(codex)).toBe(false);
    expect(live?.contains(grok)).toBe(false);
    expect(scene?.contains(codex)).toBe(true);
    expect(codex?.getAttribute("aria-hidden")).toBe("true");
    expect(grok?.getAttribute("alt")).toBe("");
    expect(root.querySelectorAll("img")).toHaveLength(3);
    expect(root.querySelector("button, a, input, textarea, select, [tabindex]")).toBeNull();
    expect(root.getAttribute("role")).toBeNull();
    expect(live?.querySelector("img, script")).toBeNull();
    expect(live?.textContent).toContain(HOSTILE);
    expect(root.querySelector(".widget-source") && live?.contains(root.querySelector(".widget-source"))).toBe(
      false,
    );
  });

  it("attributes Codex, Grok, unknown speakers, errors, and pending targets", () => {
    const paired = paint(snapshot());
    const pairedSlips = slips(paired.root);
    expect(pairedSlips.map((slip) => slip.dataset.tail)).toEqual(["toward-codex", "toward-grok"]);
    expect(pairedSlips[0]?.querySelector(".widget-excerpt")?.textContent).toBe("Ship the engine");
    expect(pairedSlips[0]?.textContent).toContain(EXTRACTED_TASK_LABEL);
    expect(pairedSlips[1]?.querySelector(".widget-excerpt")?.textContent).toBe("done & exact");
    expect(paired.root.querySelector(`#${LANDMARKS.widgetCodex}`)?.classList.contains("is-reacting")).toBe(
      true,
    );
    expect(paired.root.querySelector(`#${LANDMARKS.widgetGrok}`)?.classList.contains("is-reacting")).toBe(
      true,
    );

    const unknown = paint(
      snapshot({
        request: preview({ speaker: "Nova", recipient: "codex", excerpt: "from nova" }),
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
    );
    const unknownSlips = slips(unknown.root);
    expect(unknownSlips[0]?.dataset.tail).toBe("none");
    expect(unknownSlips[0]?.dataset.speaker).toBe("unknown");
    expect(unknownSlips[0]?.textContent).toContain("Nova");
    expect(unknownSlips[1]?.dataset.kind).toBe("parley-error");
    expect(unknownSlips[1]?.dataset.tail).toBe("none");
    expect(unknownSlips[1]?.textContent).toContain("Parley");
    expect(unknownSlips[1]?.textContent).toContain(PARLEY_ERROR_LABEL);
    expect(unknown.root.querySelector(`#${LANDMARKS.widgetGrok}`)?.classList.contains("is-reacting")).toBe(
      false,
    );

    const pending = paint(
      snapshot({
        completion: null,
        pendingLabel: PENDING_LABEL,
      }),
    );
    expect(slips(pending.root)[1]?.dataset.kind).toBe("pending");
    expect(slips(pending.root)[1]?.dataset.tail).toBe("toward-grok");
    expect(slips(pending.root)[1]?.textContent).toContain(PENDING_LABEL);

    const unaimed = paint(
      snapshot({
        request: preview({ recipient: "other" }),
        completion: null,
        pendingLabel: PENDING_LABEL,
      }),
    );
    expect(slips(unaimed.root)[1]?.dataset.tail).toBe("none");
    expect(slips(unaimed.root)[1]?.textContent).toContain("Parley");
    expect(slips(unaimed.root)).toHaveLength(2);
  });

  it("reuses message nodes for an identical revision and reacts only to a new identity", () => {
    const current = snapshot();
    const { root, scene } = paint(current);
    const excerpt = root.querySelector(".widget-excerpt");
    const source = root.querySelector(".widget-source");
    scene.paint(
      widgetSceneModel({
        status: status({ sessionCount: 4 }),
        snapshot: current,
        loadError: null,
        utc: true,
      }),
    );
    expect(root.querySelector(".widget-excerpt")).toBe(excerpt);
    expect(source?.textContent).toContain("4 sessions");

    const request = current.request;
    if (!request) {
      throw new Error("missing request");
    }
    current.request = { ...request, excerpt: "updated exact" };
    scene.paint(widgetSceneModel({ status: status(), snapshot: current, loadError: null, utc: true }));
    expect(root.querySelector(".widget-excerpt")).toBe(excerpt);
    expect(excerpt?.textContent).toBe("updated exact");

    const named = snapshot({
      request: preview({ speaker: "Nova", recipient: "codex", excerpt: "same words" }),
      completion: null,
      pendingLabel: null,
    });
    scene.paint(widgetSceneModel({ status: status(), snapshot: named, loadError: null, utc: true }));
    const name = root.querySelector(".widget-name");
    expect(name?.textContent).toBe("Nova");
    const namedRequest = named.request;
    if (!namedRequest) {
      throw new Error("missing named request");
    }
    named.request = { ...namedRequest, speaker: "Atlas" };
    scene.paint(widgetSceneModel({ status: status(), snapshot: named, loadError: null, utc: true }));
    expect(root.querySelector(".widget-name")).toBe(name);
    expect(name?.textContent).toBe("Atlas");

    current.request = { ...request, eventKey: "key-req-2", excerpt: "fresh identity" };
    scene.paint(widgetSceneModel({ status: status(), snapshot: current, loadError: null, utc: true }));
    const fresh = root.querySelector(".widget-excerpt");
    expect(fresh).not.toBe(excerpt);
    expect(fresh?.textContent).toBe("fresh identity");
    expect(fresh?.parentElement?.classList.contains("is-reacting")).toBe(true);
  });

  it("does not replace message nodes on an identical 500 ms poll", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const currentStatus = status();
    const currentSnapshot = snapshot();
    const getStatus = vi.fn(async () => currentStatus);
    const getWidgetSnapshot = vi.fn(async () => currentSnapshot);
    const api = { getStatus, getWidgetSnapshot } as unknown as ViewerApi;
    const root = document.createElement("div");
    document.body.append(root);
    const handle = mountWidget(root, api);
    await vi.waitFor(() => {
      expect(root.querySelector(".widget-excerpt")?.textContent).toBe("Ship the engine");
    });
    const excerpt = root.querySelector(".widget-excerpt");
    const messages = root.querySelector<HTMLElement>(".widget-messages");
    expect(messages?.dataset.bubbleCount).toBe("2");
    currentStatus.sessionCount = 3;
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(getWidgetSnapshot.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
    expect(root.querySelector(".widget-excerpt")).toBe(excerpt);
    expect(root.querySelector(".widget-messages")).toBe(messages);
    expect(root.querySelector(".widget-source")?.textContent).toContain("3 sessions");
    handle.stop();
  });

  it("keeps a solid conversation surface when artwork fails and skips decoration without motion", () => {
    vi.spyOn(window, "matchMedia").mockImplementation(
      (query: string) =>
        ({
          matches: query.includes("prefers-reduced-motion"),
          media: query,
          onchange: null,
          addListener: () => undefined,
          removeListener: () => undefined,
          addEventListener: () => undefined,
          removeEventListener: () => undefined,
          dispatchEvent: () => false,
        }) as unknown as MediaQueryList,
    );
    const long = "Exact bounded excerpt. ".repeat(30);
    const { root } = paint(snapshot({ request: preview({ excerpt: long }) }));
    for (const image of root.querySelectorAll("img")) {
      image.dispatchEvent(new Event("error"));
    }
    expect(root.querySelectorAll("img.is-missing")).toHaveLength(3);
    expect(root.querySelector(".widget-column")).not.toBeNull();
    expect(root.querySelector(".widget-excerpt")?.textContent).toBe(long);
    expect(root.querySelector(".widget-slip")?.classList.contains("is-reacting")).toBe(false);
    expect(root.querySelector(`#${LANDMARKS.widgetCodex}`)?.classList.contains("is-reacting")).toBe(
      false,
    );
  });

  it("shows idle and load failures as text without a second exchange", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const scene = createWidgetScene(root);
    scene.paint(widgetSceneModel({ status: null, snapshot: null, loadError: null }));
    const idle = root.querySelector(".widget-idle");
    expect(idle?.textContent).toBe("Connecting\u2026");
    expect(slips(root)).toHaveLength(0);
    scene.paint(
      widgetSceneModel({
        status: status({ sourceState: "missing" }),
        snapshot: snapshot({
          sessionKey: null,
          exchangeKey: null,
          sessionId: null,
          exchangeId: null,
          request: null,
          completion: null,
          pendingLabel: null,
        }),
        loadError: null,
      }),
    );
    expect(root.querySelector(".widget-idle")).toBe(idle);
    expect(idle?.textContent).toBe("Event log is missing");
    scene.paint(
      widgetSceneModel({
        status: status({ trayAvailable: false }),
        snapshot: null,
        loadError: null,
      }),
    );
    const banner = root.querySelector(".widget-banner");
    expect(root.querySelector(`#${LANDMARKS.widgetLive}`)?.contains(banner)).toBe(true);
    expect(banner?.textContent).toContain("tray");

    const failing = {
      getStatus: vi.fn(async () => {
        throw new Error("offline");
      }),
      getWidgetSnapshot: vi.fn(async () => snapshot()),
    } as unknown as ViewerApi;
    const mounted = document.createElement("div");
    document.body.append(mounted);
    const handle = mountWidget(mounted, failing);
    await vi.waitFor(() => {
      expect(mounted.querySelector(".widget-load-error")?.textContent).toBe(
        "Unable to load widget snapshot",
      );
    });
    expect(mounted.querySelector(`#${LANDMARKS.widgetLive}`)?.contains(
      mounted.querySelector(".widget-load-error"),
    )).toBe(true);
    handle.stop();
  });
});

describe("widget scene contract", () => {
  const css = readFileSync(resolve("src/styles/scene.css"), "utf8");

  it("uses finite opacity and transform reactions that stop at the minimum size and reduced motion", () => {
    expect(css).not.toMatch(/infinite/i);
    expect(css).not.toMatch(/text-transform/i);
    expect(css).not.toMatch(/linear-gradient|radial-gradient|cyan|purple/i);
    expect(css).toMatch(/pointer-events:\s*none/);
    expect(css).toMatch(/\.widget-figure-codex[\s\S]*grid-column:\s*1/);
    expect(css).toMatch(/\.widget-figure-grok[\s\S]*grid-column:\s*3/);
    const motion = css.match(/@keyframes widget-acknowledge \{([\s\S]*?)\n\}/);
    expect(motion?.[1]).toMatch(/opacity/);
    expect(motion?.[1]).toMatch(/transform/);
    expect(motion?.[1]).not.toMatch(/width|left|top|background|filter/);
    const compact = css.match(
      /@media \(max-width: 320px\), \(max-height: 180px\) \{([\s\S]*?)\n\}/,
    );
    expect(compact?.[1]).toMatch(/display:\s*none/);
    expect(compact?.[1]).not.toMatch(/font-size/);
    expect(css).toMatch(/prefers-reduced-motion:\s*reduce[\s\S]*animation:\s*none/);
  });

  it("keeps ink and dark marks above the text and boundary contrast floors", () => {
    const textPairs: Array<[string, string]> = [
      ["#241c16", "#f3e6d0"],
      ["#241c16", "#fff3df"],
      ["#241c16", "#dfceb4"],
      ["#5c5146", "#f3e6d0"],
      ["#5c5146", "#fff3df"],
      ["#521c17", "#e6aaa1"],
      ["#4c3100", "#f0c983"],
      ["#234e86", "#fff3df"],
      ["#8e4038", "#fff3df"],
      ["#8e4038", "#dfceb4"],
    ];
    for (const [ink, paper] of textPairs) {
      expect(contrast(ink, paper)).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrast("#234e86", "#f3e6d0")).toBeGreaterThanOrEqual(3);
    expect(contrast("#8e4038", "#f3e6d0")).toBeGreaterThanOrEqual(3);
    expect(contrast("#8a7865", "#f3e6d0")).toBeGreaterThanOrEqual(3);
  });
});

function contrast(foreground: string, background: string): number {
  const light = (hex: string): number => {
    const value = Number.parseInt(hex.slice(1), 16);
    const channel = (shift: number): number => {
      const raw = ((value >> shift) & 255) / 255;
      return raw <= 0.04045 ? raw / 12.92 : ((raw + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
  };
  const lighter = Math.max(light(foreground), light(background));
  const darker = Math.min(light(foreground), light(background));
  return (lighter + 0.05) / (darker + 0.05);
}
