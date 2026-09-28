/**
 * @vitest-environment happy-dom
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";

const tauriEvents = vi.hoisted(() => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  return {
    handlers,
    listen: vi.fn(
      async (name: string, handler: (event: { payload: unknown }) => void): Promise<() => void> => {
        handlers.set(name, handler);
        return () => {
          handlers.delete(name);
        };
      },
    ),
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: tauriEvents.listen }));

import {
  DEFAULT_PEER_HEALTH_DIAGNOSTICS,
  DEFAULT_SETTINGS,
  type EventContent,
  type ExchangeSummary,
  type HandoffSelection,
  type MessagePreview,
  type PeerActivitySnapshot,
  type PeerHealthSnapshot,
  type PeerHandoffItem,
  type PeerIncident,
  type SearchHit,
  type SessionSummary,
  type ViewerSettings,
  type ViewerStatus,
} from "../contracts";
import type { ViewerApi } from "../ipc";

import { mountDetail } from "./detail";
import { OPEN_LATEST_HANDOFF_LABEL } from "./peer-health";

const HOSTILE = "<img src=x onerror=alert(1)>";

function preview(overrides: Partial<MessagePreview> = {}): MessagePreview {
  return {
    eventKey: "event:request",
    eventId: "request-id",
    eventType: "request",
    speaker: "codex",
    recipient: "grok",
    timestampMs: 1_700_000_000_000,
    status: "ok",
    excerpt: HOSTILE,
    excerptExtracted: false,
    contentLength: 12,
    ...overrides,
  };
}

function exchange(overrides: Partial<ExchangeSummary> = {}): ExchangeSummary {
  const request = overrides.request === undefined ? preview() : overrides.request;
  return {
    exchangeKey: "exchange:1",
    sessionKey: "session:1",
    exchangeId: "exchange-1",
    sessionId: "session-1",
    sourcePath: "synthetic://events.jsonl",
    timestampMs: 1_700_000_000_000,
    request,
    completion: preview({
      eventKey: "event:completion",
      eventId: "completion-id",
      eventType: "response",
      speaker: "grok",
      recipient: "codex",
      excerpt: "Visible completion",
    }),
    pendingLabel: null,
    ...overrides,
  };
}

function status(overrides: Partial<ViewerStatus> = {}): ViewerStatus {
  return {
    sourceState: "watching",
    generation: 1,
    bytesRead: 100,
    sessionCount: 2,
    exchangeCount: 1,
    lastEventTimestampMs: 1_700_000_000_000,
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
    sources: [
      {
        path: "synthetic://events.jsonl",
        identity: "source-1",
        sourceState: "watching",
        generation: 1,
        bytesRead: 100,
        sessionCount: 2,
        exchangeCount: 1,
        lastEventTimestampMs: 1_700_000_000_000,
        diagnostics: {
          malformedLines: 0,
          oversizedLines: 0,
          unsupportedRecords: 0,
          duplicateEvents: 0,
          ioErrors: 0,
          aliasCollisions: 0,
          lastError: null,
        },
        aliasOf: null,
      },
    ],
    ...overrides,
  };
}

function incident(): PeerIncident {
  return {
    incidentId: "incident-1",
    class: "quota_exhausted",
    source: "codex",
    status: "active",
    openedMs: 1,
    asOfMs: 1,
    recoveredMs: null,
    acknowledged: false,
    sessionId: "session-1",
    eventId: "completion-id",
    exchangeId: "exchange-1",
  };
}

function handoffItem(): PeerHandoffItem {
  return {
    jobId: "job-1",
    handoffId: "handoff-1",
    sourceSessionId: "session-1",
    targetSessionId: "session-2",
    state: "awaiting_ack",
    phase: "handoff_ready",
    processState: "running",
    createdAtMs: 1,
    updatedAtMs: 1,
    lastActivityMs: 1,
    readyAtMs: 1,
    deadlineMs: 2,
    receiptAtMs: null,
    alertIncidentId: null,
    recordDiagnostic: null,
    excerptText: "Visible excerpt",
    excerptTruncated: false,
    activities: [],
    reportAvailability: "available",
    reportText: "Exact report",
  };
}

function health(unread = 1): PeerHealthSnapshot {
  return {
    schemaVersion: 3,
    generatedMs: 10,
    asOfMs: 10,
    muted: false,
    unreadCount: unread,
    latestCodexSample: null,
    latestGrokObservation: null,
    activeIncidents: unread > 0 ? [incident()] : [],
    recentIncidents: [],
    unavailable: null,
    stale: false,
    diagnostics: { ...DEFAULT_PEER_HEALTH_DIAGNOSTICS },
  };
}

function activity(): PeerActivitySnapshot {
  return {
    generatedMs: 10,
    assessment: "not_inferred",
    source: { kind: "environment" },
    unavailable: null,
    diagnostics: {
      rootMissing: false,
      rootLocked: false,
      rootMalformed: false,
      jobsMissing: false,
      jobsLocked: false,
      jobsMalformed: false,
      skippedEntries: 0,
      missingJobs: 0,
      malformedJobs: 0,
      lockedJobs: 0,
      unavailableJobs: 0,
      reportMismatches: 0,
      reportMissing: 0,
      reportLocked: 0,
      reportMalformed: 0,
    },
    shownCount: 1,
    totalCount: 1,
    truncated: false,
    handoffs: [handoffItem()],
  };
}

function sessions(): SessionSummary[] {
  return [
    {
      sessionKey: "session:1",
      sessionId: "alpha",
      sourcePath: "synthetic://events.jsonl",
      exchangeCount: 1,
      latestTimestampMs: 1_700_000_000_000,
      latestSource: "codex",
      latestTarget: "grok",
      latestExcerpt: HOSTILE,
      excerptExtracted: false,
    },
    {
      sessionKey: "session:2",
      sessionId: "beta",
      sourcePath: "synthetic://events.jsonl",
      exchangeCount: 1,
      latestTimestampMs: 1_700_000_100_000,
      latestSource: "grok",
      latestTarget: "codex",
      latestExcerpt: "Second session",
      excerptExtracted: false,
    },
  ];
}

function content(eventKey: string): EventContent {
  const secondSession = eventKey.includes("session-2");
  return {
    eventKey,
    exchangeKey: secondSession ? "exchange:2" : "exchange:1",
    sessionKey: secondSession ? "session:2" : "session:1",
    eventId: eventKey,
    exchangeId: secondSession ? "exchange-2" : "exchange-1",
    sessionId: secondSession ? "beta" : "alpha",
    sourcePath: "synthetic://events.jsonl",
    eventType: eventKey.includes("completion") ? "response" : "request",
    speaker: eventKey.includes("completion") ? "grok" : "codex",
    recipient: eventKey.includes("completion") ? "codex" : "grok",
    timestampMs: 1_700_000_000_000,
    status: "ok",
    durationMs: 12,
    error: null,
    content: eventKey.includes("completion")
      ? secondSession
        ? "Exact second-session completion body"
        : "Exact completion body"
      : `Exact request ${HOSTILE}`,
    context: null,
  };
}

function createApi(statusOverrides: Partial<ViewerStatus> = {}) {
  const calls = {
    acknowledge: [] as string[],
    mute: [] as boolean[],
    chime: 0,
    open: 0,
    exit: 0,
    content: [] as string[],
    selectLog: 0,
    removeLog: [] as string[],
    widgetVisible: [] as boolean[],
    launchAtLogin: [] as boolean[],
    savedSettings: [] as ViewerSettings[],
    retryInteractive: 0,
  };
  let bytesRead = 100;
  let currentHealth = health(1);
  const api: ViewerApi = {
    getStatus: async () => status({ ...statusOverrides, bytesRead }),
    getWidgetSnapshot: async () => ({
      sessionKey: null,
      exchangeKey: null,
      sessionId: null,
      exchangeId: null,
      request: null,
      completion: null,
      pendingLabel: null,
    }),
    getWidgetBrowser: async () => ({
      followLive: true,
      selectionState: "empty",
      position: 0,
      total: 0,
      hasOlder: false,
      hasNewer: false,
      newerCount: 0,
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
    widgetBrowseOlder: async () => api.getWidgetBrowser(),
    widgetBrowseNewer: async () => api.getWidgetBrowser(),
    widgetBrowseLive: async () => api.getWidgetBrowser(),
    openWidgetExchange: async () => undefined,
    getWidgetFeed: async () => ({
      historyToken: "test-history-v1",
      items: [],
      nextBeforeExchangeKey: null,
      hasEarlier: false,
      totalExchanges: 0,
      totalEvents: 0,
      resetRequired: false,
    }),
    getWidgetMessage: async () => null,
    openWidgetEvent: async () => undefined,
    reportWidgetSurfaceBounds: async () => status(),
    reportWidgetSurfaceActivity: async () => undefined,
    widgetSurfacePointerDown: async () => undefined,
    widgetSurfaceReady: async () => status(),
    retryInteractiveMode: async () => {
      calls.retryInteractive += 1;
      return status({
        ...statusOverrides,
        desktopRuntimeState: "interactive",
        desktopFallbackReason: null,
      });
    },
    listSessions: async () => ({ items: sessions(), nextCursor: null, total: 2 }),
    listExchanges: async (sessionKey) => ({
      items:
        sessionKey === "session:1"
          ? [exchange({ sessionKey })]
          : sessionKey === "session:2"
            ? [
                exchange({
                  exchangeKey: "exchange:2",
                  sessionKey,
                  exchangeId: "exchange-2",
                  sessionId: "beta",
                  request: preview({
                    eventKey: "event:session-2-request",
                    eventId: "session-2-request",
                    excerpt: "Second-session request",
                  }),
                  completion: preview({
                    eventKey: "event:session-2-completion",
                    eventId: "session-2-completion",
                    eventType: "response",
                    speaker: "grok",
                    recipient: "codex",
                    excerpt: "Second-session completion",
                  }),
                }),
              ]
            : [],
      nextCursor: null,
      total: sessionKey === "session:1" || sessionKey === "session:2" ? 1 : 0,
    }),
    search: async () => ({
      items: [
        hit("event:hit-1"),
        hit("event:hit-2"),
      ] satisfies SearchHit[],
      nextCursor: null,
      total: 2,
    }),
    getEventContent: async (eventKey) => {
      calls.content.push(eventKey);
      return content(eventKey);
    },
    getPeerHealth: async () => currentHealth,
    acknowledgePeerIncident: async (incidentId) => {
      calls.acknowledge.push(incidentId);
      currentHealth = health(0);
      return currentHealth;
    },
    setPeerHealthMuted: async (muted) => {
      calls.mute.push(muted);
      currentHealth = { ...currentHealth, muted };
      return currentHealth;
    },
    testPeerHealthChime: async () => {
      calls.chime += 1;
    },
    openLatestHandoff: async () => {
      calls.open += 1;
      const event = content("event:completion");
      const selection: HandoffSelection = {
        event,
        label: "Latest handoff",
        incidentId: "incident-1",
        exactUndelivered: false,
        diagnostic: null,
      };
      return selection;
    },
    getPeerActivity: async () => activity(),
    getSettings: async () => ({ ...DEFAULT_SETTINGS }),
    saveSettings: async (settings) => {
      calls.savedSettings.push(settings);
      return settings;
    },
    listMonitors: async () => [{ id: "monitor-1", name: "Main", primary: true }],
    selectEventLog: async () => {
      calls.selectLog += 1;
      return status();
    },
    setEventLog: async () => status(),
    setEventLogs: async () => status(),
    addEventLog: async () => status(),
    removeEventLog: async (path) => {
      calls.removeLog.push(path);
      return status();
    },
    setWidgetVisible: async (visible) => {
      calls.widgetVisible.push(visible);
      return status({ widgetVisible: visible });
    },
    setLaunchAtLogin: async (enabled) => {
      calls.launchAtLogin.push(enabled);
      return { ...DEFAULT_SETTINGS, launchAtLogin: enabled } satisfies ViewerSettings;
    },
    showDetail: async () => undefined,
    exit: async () => {
      calls.exit += 1;
    },
  };
  return {
    api,
    calls,
    setBytes(value: number) {
      bytesRead = value;
    },
    setHealth(snapshot: PeerHealthSnapshot) {
      currentHealth = snapshot;
    },
  };
}

function hit(eventKey: string): SearchHit {
  return {
    eventKey,
    exchangeKey: "exchange:1",
    sessionKey: "session:1",
    eventId: eventKey,
    exchangeId: "exchange-1",
    sessionId: "alpha",
    sourcePath: "synthetic://events.jsonl",
    eventType: "request",
    timestampMs: 1_700_000_000_000,
    excerpt: eventKey === "event:hit-1" ? HOSTILE : "Second hit",
    matchOffset: 3,
  };
}

async function settle(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

describe("conversation studio detail", () => {
  let stop: (() => void) | null = null;

  afterEach(() => {
    stop?.();
    stop = null;
    tauriEvents.handlers.clear();
    tauriEvents.listen.mockClear();
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    document.body.replaceChildren();
    vi.useRealTimers();
  });

  it("opens the exact widget event in the Event inspector and adopts its exchange", async () => {
    vi.useFakeTimers();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();
    await Promise.resolve();

    const handler = tauriEvents.handlers.get("widget-open-exchange");
    expect(handler).toBeDefined();
    handler?.({ payload: { eventKey: "event:session-2-completion" } });
    await settle();
    await Promise.resolve();

    expect(harness.calls.content.at(-1)).toBe("event:session-2-completion");
    expect(
      root.querySelector('[data-session-key="session:2"]')?.getAttribute("aria-selected"),
    ).toBe("true");
    expect(
      root
        .querySelector('[data-event-key="event:session-2-completion"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    expect(root.querySelector('[data-region="studio-tab-event"]')?.getAttribute("aria-selected"))
      .toBe("true");
    expect(root.querySelector(".event-body")?.textContent).toContain(
      "Exact second-session completion body",
    );
  });

  it("keeps hostile session text escaped and moves the session rail by keyboard", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    expect(root.querySelector("img.studio-logo")).not.toBeNull();
    expect(root.querySelector('[data-session-key="session:1"] img')).toBeNull();
    expect(root.textContent).toContain(HOSTILE);
    const first = root.querySelector<HTMLElement>('[data-session-key="session:1"]');
    const second = root.querySelector<HTMLElement>('[data-session-key="session:2"]');
    first?.focus();
    first?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement).toBe(second);
    expect(first?.getAttribute("aria-selected")).toBe("false");
    second?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    expect(root.querySelector('[data-session-key="session:2"]')?.getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("searches from the timeline, retains focus across a refresh, and does not leave the search field", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const input = root.querySelector<HTMLInputElement>('input[type="search"]');
    expect(input).not.toBeNull();
    input!.value = "exact";
    input!.focus();
    input!.form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    const firstHit = root.querySelector<HTMLElement>('[data-event-key="event:hit-1"]');
    const secondHit = root.querySelector<HTMLElement>('[data-event-key="event:hit-2"]');
    expect(firstHit?.textContent).toContain(HOSTILE);
    expect(root.querySelector('[data-event-key="event:hit-1"] img')).toBeNull();
    firstHit?.focus();
    firstHit?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement).toBe(secondHit);

    const previousHit = document.activeElement;
    harness.setBytes(240);
    await vi.advanceTimersByTimeAsync(500);
    expect(previousHit?.isConnected).toBe(false);
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:hit-2");
    expect(root.querySelector('[data-event-key="event:hit-2"]')?.getAttribute("aria-selected")).toBe(
      "false",
    );

    const search = root.querySelector<HTMLInputElement>('input[type="search"]');
    search?.focus();
    harness.setBytes(280);
    await vi.advanceTimersByTimeAsync(500);
    expect(document.activeElement).toBe(search);
  });

  it("keeps activity counts from selecting a tab and still calls the existing helper actions", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const eventTab = root.querySelector<HTMLButtonElement>('[data-region="studio-tab-event"]');
    const activityTab = root.querySelector<HTMLButtonElement>('[data-region="studio-tab-activity"]');
    expect(eventTab?.getAttribute("aria-selected")).toBe("true");
    expect(activityTab?.textContent).toContain("1 unread, 1 pending");

    harness.setHealth({
      ...health(1),
      unreadCount: 2,
      activeIncidents: [incident(), { ...incident(), incidentId: "incident-2" }],
    });
    await vi.advanceTimersByTimeAsync(500);
    expect(eventTab?.getAttribute("aria-selected")).toBe("true");
    expect(activityTab?.textContent).toContain("2 unread, 1 pending");

    activityTab?.click();
    expect(activityTab?.getAttribute("aria-selected")).toBe("true");
    root.querySelector<HTMLButtonElement>(".peer-health-ack")?.click();
    await settle();
    expect(harness.calls.acknowledge).toEqual(["incident-1"]);
    expect(activityTab?.getAttribute("aria-selected")).toBe("true");

    const mute = root.querySelector<HTMLInputElement>("#peer-health-muted");
    mute!.checked = true;
    mute!.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(harness.calls.mute).toEqual([true]);

    root.querySelector<HTMLButtonElement>('button[aria-label="Test chime"]')?.click();
    await settle();
    expect(harness.calls.chime).toBe(1);

    root
      .querySelector<HTMLButtonElement>(`button[aria-label="${OPEN_LATEST_HANDOFF_LABEL}"]`)
      ?.click();
    await settle();
    expect(harness.calls.open).toBe(1);
    expect(eventTab?.getAttribute("aria-selected")).toBe("true");
    expect(root.querySelector(".event-body")?.textContent).toContain("Exact completion body");

    const exit = root.querySelector<HTMLButtonElement>('[data-region="studio-exit"]');
    const banner = root.querySelector<HTMLElement>('[data-region="studio-banner"]');
    expect(exit?.closest('[role="tabpanel"]')).toBeNull();
    expect(banner?.closest('[role="tabpanel"]')).toBeNull();
    exit?.click();
    await settle();
    expect(harness.calls.exit).toBe(1);
  });

  it("keeps source, placement, visibility, and autostart controls connected", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    root.querySelector<HTMLButtonElement>('[data-region="studio-tab-sources"]')?.click();
    const sourcePanel = root.querySelector<HTMLElement>('[role="tabpanel"]:not([hidden])');
    sourcePanel?.querySelector<HTMLButtonElement>(".studio-action")?.click();
    await settle();
    expect(harness.calls.selectLog).toBe(1);

    root.querySelector<HTMLButtonElement>('[data-region="studio-tab-sources"]')?.click();
    root.querySelector<HTMLButtonElement>(".studio-source-remove")?.click();
    await settle();
    expect(harness.calls.removeLog).toEqual(["synthetic://events.jsonl"]);

    root.querySelector<HTMLButtonElement>('[data-region="studio-tab-settings"]')?.click();
    const corner = root.querySelector<HTMLSelectElement>("#corner");
    const horizontal = root.querySelector<HTMLInputElement>("#offset-x");
    const vertical = root.querySelector<HTMLInputElement>("#offset-y");
    const width = root.querySelector<HTMLInputElement>("#width");
    const height = root.querySelector<HTMLInputElement>("#height");
    const desktopMode = root.querySelector<HTMLSelectElement>("#desktop-mode");
    corner!.value = "top-left";
    horizontal!.value = "30";
    vertical!.value = "32";
    width!.value = "600";
    height!.value = "380";
    desktopMode!.value = "passive";
    horizontal!.dispatchEvent(new Event("input", { bubbles: true }));
    horizontal!.form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(harness.calls.savedSettings.at(-1)).toMatchObject({
      corner: "top-left",
      offsetX: 30,
      offsetY: 32,
      width: 600,
      height: 380,
      desktopMode: "passive",
    });

    const widgetVisible = root.querySelector<HTMLInputElement>("#widget-visible");
    widgetVisible!.checked = false;
    widgetVisible!.dispatchEvent(new Event("change", { bubbles: true }));
    const launchAtLogin = root.querySelector<HTMLInputElement>("#launch-at-login");
    launchAtLogin!.checked = true;
    launchAtLogin!.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(harness.calls.widgetVisible).toEqual([false]);
    expect(harness.calls.launchAtLogin).toEqual([true]);
  });

  it("offers an explicit retry only while interactive mode is in fallback", async () => {
    vi.useFakeTimers();
    const harness = createApi({
      desktopRuntimeState: "passive-fallback",
      desktopFallbackReason: "surface-z-order-invalid",
    });
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    root.querySelector<HTMLButtonElement>('[data-region="studio-tab-settings"]')?.click();
    const retry = Array.from(root.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Retry interactive mode",
    );
    expect(retry?.disabled).toBe(false);
    expect(root.textContent).toContain("surface z order invalid");

    retry?.click();
    await settle();

    expect(harness.calls.retryInteractive).toBe(1);
    expect(retry?.disabled).toBe(true);
    expect(root.textContent).toContain("Desktop mode: interactive");
  });
});

describe("studio presentation contract", () => {
  it("uses paper tokens, a 44px target, a 3px focus, and no infinite or glow language", () => {
    const css = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "../styles/studio.css"),
      "utf8",
    );
    expect(contrast("#241c16", "#f3e6d0")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#5c5146", "#f3e6d0")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#234e86", "#f3e6d0")).toBeGreaterThanOrEqual(3);
    expect(contrast("#4c3100", "#f0c983")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#521c17", "#e6aaa1")).toBeGreaterThanOrEqual(4.5);
    expect(css).toContain("var(--paper)");
    expect(css).toContain("color: var(--ink)");
    expect(css).toContain("outline: 3px solid var(--focus-on-paper)");
    expect(css).toContain("min-height: var(--target-min)");
    expect(css).toContain("font-family: var(--exact-font)");
    expect(css).toContain("@container (max-width: 53em)");
    expect(css).toContain("grid-column: 1 / -1");
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
    expect(css).not.toMatch(/infinite/i);
    expect(css).not.toMatch(/uppercase/);
    expect(css).not.toMatch(/glow/i);
  });
});

function contrast(foreground: string, background: string): number {
  const [lighter, darker] = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
  return ((lighter ?? 0) + 0.05) / ((darker ?? 0) + 0.05);
}

function luminance(hex: string): number {
  const value = hex.replace("#", "");
  const channels = [0, 2, 4].map((offset) => {
    const channel = Number.parseInt(value.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * (channels[0] ?? 0) + 0.7152 * (channels[1] ?? 0) + 0.0722 * (channels[2] ?? 0);
}
