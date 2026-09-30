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
  type ContinuationStatus,
  type EventContent,
  type ExchangeSummary,
  type HandoffSelection,
  type MessagePreview,
  type PeerActivitySnapshot,
  type PeerHealthSnapshot,
  type PeerHandoffItem,
  type PeerIncident,
  type SearchHit,
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

function continuationStatus(overrides: Partial<ContinuationStatus> = {}): ContinuationStatus {
  return {
    schemaVersion: 1,
    state: "absent",
    message: "No published continuation is available",
    manifestId: null,
    worktreeName: null,
    writablePaths: [],
    expiresAtMs: null,
    continuationCount: 0,
    runningContinuationId: null,
    latest: null,
    ...overrides,
  };
}

function completedContinuation(completionEventId = "completion-id"): ContinuationStatus {
  return continuationStatus({
    state: "completed",
    manifestId: "manifest-1",
    worktreeName: "synthetic-worktree",
    writablePaths: ["viewer/src/ui"],
    continuationCount: 1,
    latest: {
      schemaVersion: 1,
      continuationId: "123e4567-e89b-12d3-a456-426614174000",
      manifestId: "manifest-1",
      state: "completed",
      prompt: "Private duplicate prompt",
      reply: "Private duplicate reply",
      error: null,
      exchangeId: "exchange-1",
      requestEventId: "request-id",
      completionEventId,
      changedPaths: ["viewer/src/ui/detail.ts"],
      validationState: "not_executed",
      diffFingerprint: "diff-fingerprint",
      evidenceHash: "evidence-hash",
      completedAtMs: 1_700_000_000_500,
      userReceived: false,
    },
  });
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

function createApi(
  statusOverrides: Partial<ViewerStatus> = {},
  options: {
    pagedConversation?: boolean;
    continuation?: ContinuationStatus;
    conversation?: ExchangeSummary[];
  } = {},
) {
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
    receive: [] as Array<{ continuationId: string; evidenceHash: string }>,
    conversation: [] as Array<{
      beforeExchangeKey: string | null;
      aroundExchangeKey: string | null;
    }>,
  };
  let bytesRead = 100;
  let currentHealth = health(1);
  let currentContinuation = options.continuation ?? continuationStatus();
  const conversation = options.conversation ?? [
    exchange({ sessionKey: "session:1" }),
    exchange({
      exchangeKey: "exchange:2",
      sessionKey: "session:2",
      exchangeId: "exchange-2",
      sessionId: "beta",
      timestampMs: 1_700_000_100_000,
      request: preview({
        eventKey: "event:session-2-request",
        eventId: "session-2-request",
        timestampMs: 1_700_000_100_000,
        excerpt: "Second-session request",
      }),
      completion: preview({
        eventKey: "event:session-2-completion",
        eventId: "session-2-completion",
        eventType: "response",
        speaker: "grok",
        recipient: "codex",
        timestampMs: 1_700_000_100_500,
        excerpt: "Second-session completion",
      }),
    }),
  ].reverse();
  const api: ViewerApi = {
    getStatus: async () => status({ ...statusOverrides, bytesRead }),
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
    listSessions: async () => ({ items: [], nextCursor: null, total: 0 }),
    listExchanges: async () => ({ items: [], nextCursor: null, total: 0 }),
    getConversationPage: async (beforeExchangeKey, aroundExchangeKey) => {
      calls.conversation.push({ beforeExchangeKey, aroundExchangeKey });
      const paged = options.pagedConversation === true;
      const items = aroundExchangeKey
        ? conversation.filter((item) => item.exchangeKey === aroundExchangeKey)
        : beforeExchangeKey === "exchange:2"
          ? conversation.slice(1)
          : paged
            ? conversation.slice(0, 1)
            : conversation;
      return {
        historyToken: "conversation-v1",
        items,
        nextBeforeExchangeKey:
          paged && !aroundExchangeKey && !beforeExchangeKey ? "exchange:2" : null,
        hasEarlier: paged && !aroundExchangeKey && !beforeExchangeKey,
        hasNewer: aroundExchangeKey === "exchange:1" || beforeExchangeKey != null,
        totalExchanges: conversation.length,
        anchorExchangeKey: aroundExchangeKey,
        resetRequired: false,
      };
    },
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
    getContinuationStatus: async () => currentContinuation,
    submitContinuation: async () => ({
      continuationId: "123e4567-e89b-12d3-a456-426614174000",
      state: "completed",
    }),
    cancelContinuation: async () => currentContinuation,
    receiveContinuation: async (continuationId, evidenceHash) => {
      calls.receive.push({ continuationId, evidenceHash });
      if (currentContinuation.latest?.continuationId === continuationId) {
        currentContinuation = {
          ...currentContinuation,
          latest: { ...currentContinuation.latest, userReceived: true },
        };
      }
      return currentContinuation;
    },
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

function chooseUtility(root: HTMLElement, utility: "activity" | "sources" | "settings"): void {
  root.querySelector<HTMLButtonElement>('[data-region="studio-utilities"]')?.click();
  root.querySelector<HTMLButtonElement>(`[role="menuitem"][data-utility="${utility}"]`)?.click();
}

describe("conversation studio detail", () => {
  let stop: (() => void) | null = null;

  afterEach(() => {
    stop?.();
    stop = null;
    tauriEvents.handlers.clear();
    tauriEvents.listen.mockClear();
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    Reflect.deleteProperty(window, "matchMedia");
    Reflect.deleteProperty(document, "visibilityState");
    document.body.replaceChildren();
    vi.useRealTimers();
  });

  it("opens the exact widget event in the closed-by-default evidence drawer", async () => {
    vi.useFakeTimers();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const harness = createApi({}, { pagedConversation: true });
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();
    await Promise.resolve();

    const handler = tauriEvents.handlers.get("widget-open-exchange");
    expect(handler).toBeDefined();
    handler?.({ payload: { eventKey: "event:request" } });
    await settle();
    await Promise.resolve();

    expect(harness.calls.content.at(-1)).toBe("event:request");
    expect(harness.calls.conversation.at(-1)).toEqual({
      beforeExchangeKey: null,
      aroundExchangeKey: "exchange:1",
    });
    expect(root.querySelector('[aria-label="Sessions"]')).toBeNull();
    expect(root.querySelector('[data-event-key="event:request"]')?.getAttribute("aria-current"))
      .toBe("true");
    const drawer = root.querySelector<HTMLElement>('[data-region="studio-drawer"]');
    expect(drawer?.hidden).toBe(false);
    expect(drawer?.getAttribute("role")).toBe("complementary");
    expect(drawer?.querySelector(".studio-focus-exact")?.textContent).toContain("Exact request");
    expect(drawer?.querySelector("details")?.hasAttribute("open")).toBe(false);
    expect(root.querySelector('[role="tablist"]')).toBeNull();
  });

  it("appends earlier exchanges without restoring a session boundary or moving the viewport", async () => {
    vi.useFakeTimers();
    const harness = createApi({}, { pagedConversation: true });
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const transcript = root.querySelector<HTMLElement>('[data-region="studio-transcript"]');
    expect(transcript?.textContent).not.toContain(HOSTILE);
    const loadEarlier = Array.from(transcript?.querySelectorAll<HTMLButtonElement>("button") ?? [])
      .find((button) => button.textContent === "Load earlier messages");
    expect(loadEarlier?.disabled).toBe(false);
    const scrollport = transcript?.querySelector<HTMLElement>(
      '[data-region="studio-transcript-scroll"]',
    );
    expect(scrollport).not.toBeNull();
    if (scrollport) {
      scrollport.scrollTop = 73;
    }
    loadEarlier?.click();
    await settle();

    const text = transcript?.textContent ?? "";
    expect(text).toContain(HOSTILE);
    expect(text).toContain("Second-session request");
    expect(text.indexOf("Second-session request")).toBeLessThan(text.indexOf(HOSTILE));
    expect(scrollport?.scrollTop).toBe(73);
    expect(root.querySelector('[aria-label="Sessions"]')).toBeNull();
  });

  it("renders every session as one escaped newest-first transcript without diagnostics", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const transcript = root.querySelector<HTMLElement>('[data-region="studio-transcript"]');
    expect(transcript?.querySelector("img")).toBeNull();
    expect(transcript?.textContent).toContain(HOSTILE);
    expect(transcript?.textContent).toContain("Second-session request");
    const transcriptText = transcript?.textContent ?? "";
    expect(transcriptText.indexOf("Second-session request")).toBeLessThan(
      transcriptText.indexOf(HOSTILE),
    );
    expect(transcriptText.indexOf("Second-session request")).toBeLessThan(
      transcriptText.indexOf("Second-session completion"),
    );
    expect(transcript?.textContent).not.toContain("alpha");
    expect(transcript?.textContent).not.toContain("beta");
    expect(transcript?.textContent).not.toContain("synthetic://events.jsonl");
    expect(root.querySelector('[aria-label="Sessions"]')).toBeNull();
    expect(root.querySelector('[data-region="studio-drawer"]')?.hasAttribute("hidden")).toBe(true);
  });

  it("searches without fetching on focus and retains semantic focus across polling", async () => {
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
    expect(harness.calls.content).toEqual([]);

    const focus = vi.spyOn(HTMLElement.prototype, "focus");
    harness.setBytes(240);
    await vi.advanceTimersByTimeAsync(500);
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:hit-2");
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(harness.calls.content).toEqual([]);

    document.activeElement?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    await settle();
    expect(harness.calls.content).toEqual(["event:hit-2"]);

    const search = root.querySelector<HTMLInputElement>('input[type="search"]');
    search?.focus();
    harness.setBytes(280);
    await vi.advanceTimersByTimeAsync(500);
    expect(document.activeElement).toBe(search);
  });

  it("keeps utility counts quiet until explicitly opened and preserves helper actions", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const utilities = root.querySelector<HTMLButtonElement>('[data-region="studio-utilities"]');
    const drawer = root.querySelector<HTMLElement>('[data-region="studio-drawer"]');
    expect(utilities?.getAttribute("aria-label")).toBe("Utilities, 1 unread, 1 pending");
    expect(drawer?.hidden).toBe(true);

    harness.setHealth({
      ...health(1),
      unreadCount: 2,
      activeIncidents: [incident(), { ...incident(), incidentId: "incident-2" }],
    });
    await vi.advanceTimersByTimeAsync(500);
    expect(utilities?.getAttribute("aria-label")).toBe("Utilities, 2 unread, 1 pending");
    expect(drawer?.hidden).toBe(true);

    chooseUtility(root, "activity");
    await Promise.resolve();
    expect(drawer?.hidden).toBe(false);
    expect(drawer?.querySelector("h2")?.textContent).toBe("Activity");
    root.querySelector<HTMLButtonElement>(".peer-health-ack")?.click();
    await settle();
    expect(harness.calls.acknowledge).toEqual(["incident-1"]);

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
    expect(drawer?.querySelector("h2")?.textContent).toBe("Exact event evidence");
    expect(drawer?.querySelector(".studio-focus-exact")?.textContent).toContain(
      "Exact completion body",
    );

    const exit = root.querySelector<HTMLButtonElement>('[data-region="studio-exit"]');
    const banner = root.querySelector<HTMLElement>('[data-region="studio-banner"]');
    expect(exit?.closest('[role="tabpanel"]')).toBeNull();
    expect(banner?.closest('[role="tabpanel"]')).toBeNull();
    exit?.click();
    await settle();
    expect(harness.calls.exit).toBe(1);
  });

  it("keeps source, placement, visibility, and autostart controls in the utility drawer", async () => {
    vi.useFakeTimers();
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    chooseUtility(root, "sources");
    await Promise.resolve();
    const sourcePanel = root.querySelector<HTMLElement>(
      '.studio-focus-utility-panel[data-utility="sources"]',
    );
    sourcePanel?.querySelector<HTMLButtonElement>(".studio-action")?.click();
    await settle();
    expect(harness.calls.selectLog).toBe(1);

    chooseUtility(root, "sources");
    await Promise.resolve();
    root.querySelector<HTMLButtonElement>(".studio-source-remove")?.click();
    await settle();
    expect(harness.calls.removeLog).toEqual(["synthetic://events.jsonl"]);

    chooseUtility(root, "settings");
    await Promise.resolve();
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

  it("records one receipt only after the matching terminal row is visibly presented", async () => {
    vi.useFakeTimers();
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    const harness = createApi({}, { continuation: completedContinuation() });
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    expect(harness.calls.receive).toEqual([]);
    const scrollport = root.querySelector<HTMLElement>('[data-region="studio-transcript-scroll"]');
    const row = root.querySelector<HTMLElement>('[data-event-key="event:completion"]');
    expect(scrollport).not.toBeNull();
    expect(row).not.toBeNull();
    Object.defineProperty(scrollport, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ top: 0, bottom: 240, left: 0, right: 600, width: 600, height: 240 }),
    });
    Object.defineProperty(row, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ top: 20, bottom: 80, left: 0, right: 600, width: 600, height: 60 }),
    });
    scrollport?.dispatchEvent(new Event("scroll"));
    await settle();

    expect(harness.calls.receive).toEqual([
      {
        continuationId: "123e4567-e89b-12d3-a456-426614174000",
        evidenceHash: "evidence-hash",
      },
    ]);
    scrollport?.dispatchEvent(new Event("scroll"));
    await settle();
    expect(harness.calls.receive).toHaveLength(1);
  });

  it("shows exact evidence in a compact modal with inert background and restores row focus", async () => {
    vi.useFakeTimers();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: () => ({
        matches: true,
        media: "(max-width: 53em)",
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      }),
    });
    const harness = createApi();
    const root = document.createElement("div");
    document.body.append(root);
    stop = mountDetail(root, harness.api).stop;
    await settle();

    const row = root.querySelector<HTMLButtonElement>('[data-event-key="event:completion"]');
    row?.focus();
    row?.click();
    await settle();
    const drawer = root.querySelector<HTMLElement>('[data-region="studio-drawer"]');
    expect(drawer?.getAttribute("role")).toBe("dialog");
    expect(drawer?.getAttribute("aria-modal")).toBe("true");
    expect(root.querySelector('[data-region="studio-header"]')?.hasAttribute("inert")).toBe(true);
    expect(root.querySelector(".studio-focus-main")?.hasAttribute("inert")).toBe(true);

    drawer?.querySelector<HTMLButtonElement>(".studio-focus-close")?.click();
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:completion");
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

    chooseUtility(root, "settings");
    await Promise.resolve();
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
    const styles = dirname(fileURLToPath(import.meta.url));
    const css = ["studio.css", "studio-focus.css"]
      .map((name) => readFileSync(join(styles, "../styles", name), "utf8"))
      .join("\n");
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
    expect(css).toContain(".studio-focus-workspace:has([data-region=\"studio-drawer\"].is-wide)");
    expect(css).toContain("grid-template-columns: minmax(0, 1fr) minmax(18rem, 26rem)");
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
