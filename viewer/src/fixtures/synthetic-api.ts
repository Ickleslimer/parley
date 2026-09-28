import {
  DEFAULT_PEER_HEALTH_DIAGNOSTICS,
  DEFAULT_SETTINGS,
  type EventContent,
  type ExchangeSummary,
  type MessagePreview,
  type PeerActivitySnapshot,
  type PeerHealthSnapshot,
  type SearchHit,
  type ViewerSettings,
  type ViewerStatus,
  type WidgetBrowserSnapshot,
  type WidgetFeedExchange,
  type WidgetFeedMessage,
  type WidgetFeedPage,
  type WidgetSnapshot,
} from "../contracts";
import type { ViewerApi } from "../ipc";
import type { FixtureScenario } from "./contracts";

const NOW = Date.UTC(2026, 8, 27, 9, 30, 0);
const SOURCE = "synthetic://two-chairs/events.jsonl";
const SESSION_KEY = "session:synthetic";
const SESSION_ID = "synthetic-session";
const EXCHANGE_KEY = "exchange:synthetic";
const EXCHANGE_ID = "synthetic-exchange";
const PENDING_LABEL = "Request logged; no response event yet";

const EMPTY_DIAGNOSTICS = {
  malformedLines: 0,
  oversizedLines: 0,
  unsupportedRecords: 0,
  duplicateEvents: 0,
  ioErrors: 0,
  aliasCollisions: 0,
  lastError: null,
};

export interface SyntheticFixture {
  api: ViewerApi;
  status: ViewerStatus;
  widget: WidgetSnapshot;
  exchanges: ExchangeSummary[];
  events: Map<string, EventContent>;
}

export function createSyntheticFixture(scenario: FixtureScenario): SyntheticFixture {
  const request = preview({
    eventKey: "event:request",
    eventId: "synthetic-request",
    eventType: "request",
    speaker:
      scenario === "reversed-route" ? "grok" : scenario === "unknown-agent" ? "nova" : "codex",
    recipient: scenario === "reversed-route" ? "codex" : "grok",
    timestampMs: NOW,
    excerpt:
      scenario === "maximum-exchange"
        ? maximumExcerpt("Review the assembled evidence without rewriting any exact record. ")
        : scenario === "unknown-agent"
          ? "Can the two chairs make room for a visiting reviewer?"
          : "Check the new conversation studio against the frozen design contract.",
    excerptExtracted: true,
  });
  const completion =
    scenario === "pending" || scenario === "idle"
      ? null
      : preview({
          eventKey: "event:completion",
          eventId: "synthetic-completion",
          eventType: scenario === "error" ? "error" : "response",
          speaker: scenario === "reversed-route" ? "codex" : scenario === "error" ? "codex" : "grok",
          recipient: scenario === "reversed-route" ? "grok" : "codex",
          timestampMs: NOW + 42_000,
          status: scenario === "error" ? "failed" : "ok",
          excerpt:
            scenario === "error"
              ? "Synthetic transport failure. No model speech was delivered."
              : scenario === "maximum-exchange"
                ? maximumExcerpt("The evidence remains exact, readable, and attributed after the full review. ")
                : "The paper surfaces preserve exact content and the controls remain keyboard reachable.",
          excerptExtracted: false,
        });
  const exchange: ExchangeSummary = {
    exchangeKey: EXCHANGE_KEY,
    sessionKey: SESSION_KEY,
    exchangeId: EXCHANGE_ID,
    sessionId: SESSION_ID,
    sourcePath: SOURCE,
    timestampMs: completion?.timestampMs ?? request.timestampMs,
    request,
    completion,
    pendingLabel: completion || scenario === "idle" ? null : PENDING_LABEL,
  };
  const populatedWidget: WidgetSnapshot = {
    sessionKey: SESSION_KEY,
    exchangeKey: EXCHANGE_KEY,
    sessionId: SESSION_ID,
    exchangeId: EXCHANGE_ID,
    request,
    completion,
    pendingLabel: completion ? null : PENDING_LABEL,
  };
  const widget: WidgetSnapshot =
    scenario === "idle" ||
    scenario === "empty" ||
    scenario === "missing-selection" ||
    scenario === "ambiguous"
      ? {
          sessionKey: null,
          exchangeKey: null,
          sessionId: null,
          exchangeId: null,
          request: null,
          completion: null,
          pendingLabel: null,
        }
      : populatedWidget;
  const sourceError = scenario === "source-error";
  const empty = scenario === "empty";
  const status: ViewerStatus = {
    sourceState: sourceError ? "degraded" : "watching",
    generation: 1,
    bytesRead: 4_096,
    sessionCount: empty ? 0 : 1,
    exchangeCount: empty ? 0 : 1,
    lastEventTimestampMs: empty ? null : exchange.timestampMs,
    trayAvailable: true,
    underlayState: "attached",
    desktopRuntimeState: scenario === "passive-fallback" ? "passive-fallback" : "interactive",
    desktopFallbackReason: scenario === "passive-fallback" ? "surface-z-order-invalid" : null,
    widgetVisible: true,
    diagnostics: {
      ...EMPTY_DIAGNOSTICS,
      ioErrors: sourceError ? 1 : 0,
      lastError: sourceError ? "Synthetic source read failure" : null,
    },
    sources: [
      {
        path: SOURCE,
        identity: "synthetic-source",
        sourceState: sourceError ? "degraded" : "watching",
        generation: 1,
        bytesRead: 4_096,
        sessionCount: empty ? 0 : 1,
        exchangeCount: empty ? 0 : 1,
        lastEventTimestampMs: empty ? null : exchange.timestampMs,
        diagnostics: {
          ...EMPTY_DIAGNOSTICS,
          ioErrors: sourceError ? 1 : 0,
          lastError: sourceError ? "Synthetic source read failure" : null,
        },
        aliasOf: null,
      },
    ],
  };
  const exchanges = empty || scenario === "idle" ? [] : [exchange];
  const events = new Map<string, EventContent>();
  if (!empty && scenario !== "idle") {
    events.set(request.eventKey, content(request, request.excerpt));
    if (completion) {
      events.set(
        completion.eventKey,
        content(
          completion,
          scenario === "maximum-exchange"
            ? `${completion.excerpt}\n\n${"Exact synthetic body. ".repeat(3_200)}`
            : completion.excerpt,
        ),
      );
    }
  }

  let settings: ViewerSettings = { ...DEFAULT_SETTINGS };
  let mutableStatus = status;
  let peerHealth = syntheticPeerHealth();
  const activity = syntheticPeerActivity();
  const searchHits: SearchHit[] = exchanges.length
    ? [
        {
          eventKey: request.eventKey,
          exchangeKey: EXCHANGE_KEY,
          sessionKey: SESSION_KEY,
          eventId: request.eventId,
          exchangeId: EXCHANGE_ID,
          sessionId: SESSION_ID,
          sourcePath: SOURCE,
          eventType: request.eventType,
          timestampMs: request.timestampMs,
          excerpt: request.excerpt,
          matchOffset: 0,
        },
      ]
    : [];
  const feed = buildSyntheticFeed(scenario);
  const api: ViewerApi = {
    getStatus: async () => mutableStatus,
    getWidgetSnapshot: async () => widget,
    getWidgetBrowser: async () => browserFor(scenario, widget),
    widgetBrowseOlder: async () => api.getWidgetBrowser(),
    widgetBrowseNewer: async () => api.getWidgetBrowser(),
    widgetBrowseLive: async () => api.getWidgetBrowser(),
    openWidgetExchange: async () => undefined,
    getWidgetFeed: async (beforeExchangeKey) => feed.page(beforeExchangeKey),
    getWidgetMessage: async (eventKey) => feed.message(eventKey),
    openWidgetEvent: async () => undefined,
    reportWidgetSurfaceBounds: async () => mutableStatus,
    reportWidgetSurfaceActivity: async () => undefined,
    widgetSurfacePointerDown: async () => undefined,
    widgetSurfaceReady: async () => mutableStatus,
    retryInteractiveMode: async () => mutableStatus,
    listSessions: async () => ({
      items: exchanges.length
        ? [
            {
              sessionKey: SESSION_KEY,
              sessionId: SESSION_ID,
              sourcePath: SOURCE,
              exchangeCount: 1,
              latestTimestampMs: exchange.timestampMs,
              latestSource: request.speaker,
              latestTarget: request.recipient,
              latestExcerpt: request.excerpt,
              excerptExtracted: true,
            },
          ]
        : [],
      nextCursor: null,
      total: exchanges.length ? 1 : 0,
    }),
    listExchanges: async () => ({ items: exchanges, nextCursor: null, total: exchanges.length }),
    search: async () => ({ items: searchHits, nextCursor: null, total: searchHits.length }),
    getEventContent: async (eventKey) => events.get(eventKey) ?? null,
    getPeerHealth: async () => peerHealth,
    acknowledgePeerIncident: async () => peerHealth,
    setPeerHealthMuted: async (muted) => {
      peerHealth = { ...peerHealth, muted };
      return peerHealth;
    },
    testPeerHealthChime: async () => undefined,
    openLatestHandoff: async () => ({
      event: completion ? events.get(completion.eventKey) ?? null : null,
      label: "Synthetic latest context",
      incidentId: null,
      exactUndelivered: false,
      diagnostic: null,
    }),
    getPeerActivity: async () => activity,
    getSettings: async () => settings,
    saveSettings: async (next) => {
      settings = { ...next };
      return settings;
    },
    listMonitors: async () => [{ id: "synthetic-monitor", name: "Synthetic monitor", primary: true }],
    selectEventLog: async () => mutableStatus,
    setEventLog: async () => mutableStatus,
    setEventLogs: async () => mutableStatus,
    addEventLog: async () => mutableStatus,
    removeEventLog: async () => mutableStatus,
    setWidgetVisible: async (visible) => {
      mutableStatus = { ...mutableStatus, widgetVisible: visible };
      return mutableStatus;
    },
    setLaunchAtLogin: async (enabled) => {
      settings = { ...settings, launchAtLogin: enabled };
      return settings;
    },
    showDetail: async () => undefined,
    exit: async () => undefined,
  };
  return { api, status, widget, exchanges, events };
}

function browserFor(scenario: FixtureScenario, widget: WidgetSnapshot): WidgetBrowserSnapshot {
  if (scenario === "historical") {
    return {
      followLive: false,
      selectionState: "selected",
      position: 2,
      total: 5,
      hasOlder: true,
      hasNewer: true,
      newerCount: 2,
      widget,
    };
  }
  if (scenario === "missing-selection") {
    return {
      followLive: false,
      selectionState: "missing",
      position: 1,
      total: 4,
      hasOlder: true,
      hasNewer: true,
      newerCount: 1,
      widget,
    };
  }
  if (scenario === "ambiguous") {
    return {
      followLive: false,
      selectionState: "ambiguous",
      position: 1,
      total: 4,
      hasOlder: true,
      hasNewer: false,
      newerCount: 0,
      widget,
    };
  }
  const selected = widget.exchangeKey != null;
  return {
    followLive: true,
    selectionState: selected ? "selected" : "empty",
    position: 0,
    total: selected ? 1 : 0,
    hasOlder: false,
    hasNewer: false,
    newerCount: 0,
    widget,
  };
}

function preview(overrides: Partial<MessagePreview>): MessagePreview {
  const excerpt = overrides.excerpt ?? "Synthetic message";
  return {
    eventKey: "event:synthetic",
    eventId: "synthetic-event",
    eventType: "request",
    speaker: "codex",
    recipient: "grok",
    timestampMs: NOW,
    status: "ok",
    excerpt,
    excerptExtracted: false,
    contentLength: Array.from(excerpt).length,
    ...overrides,
  };
}

function content(message: MessagePreview, body: string): EventContent {
  return {
    eventKey: message.eventKey,
    exchangeKey: EXCHANGE_KEY,
    sessionKey: SESSION_KEY,
    eventId: message.eventId,
    exchangeId: EXCHANGE_ID,
    sessionId: SESSION_ID,
    sourcePath: SOURCE,
    eventType: message.eventType,
    speaker: message.speaker,
    recipient: message.recipient,
    timestampMs: message.timestampMs,
    status: message.status,
    durationMs: message.eventType === "request" ? null : 42_000,
    error: message.eventType === "error" ? "Synthetic transport failure" : null,
    content: body,
    context: null,
  };
}

function maximumExcerpt(seed: string): string {
  return Array.from(seed.repeat(20)).slice(0, 420).join("");
}

function syntheticPeerHealth(): PeerHealthSnapshot {
  return {
    schemaVersion: 3,
    generatedMs: NOW,
    asOfMs: NOW,
    muted: false,
    unreadCount: 0,
    latestCodexSample: {
      usedPercent: 31,
      resetsAt: "2026-09-28T00:00:00Z",
      planType: "synthetic",
      rateLimitReachedType: null,
      asOfMs: NOW,
    },
    latestGrokObservation: {
      class: "usage_sample",
      asOfMs: NOW,
      success: true,
      httpStatus: null,
      providerCode: null,
    },
    activeIncidents: [],
    recentIncidents: [],
    unavailable: null,
    stale: false,
    diagnostics: { ...DEFAULT_PEER_HEALTH_DIAGNOSTICS },
  };
}

function syntheticPeerActivity(): PeerActivitySnapshot {
  return {
    generatedMs: NOW,
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
    handoffs: [
      {
        jobId: "synthetic-job",
        handoffId: "synthetic-handoff",
        sourceSessionId: "synthetic-codex-session",
        targetSessionId: "synthetic-grok-session",
        state: "awaiting_ack",
        phase: "handoff_ready",
        processState: "running",
        createdAtMs: NOW - 60_000,
        updatedAtMs: NOW,
        lastActivityMs: NOW,
        readyAtMs: NOW,
        deadlineMs: NOW + 21_600_000,
        receiptAtMs: null,
        alertIncidentId: null,
        recordDiagnostic: null,
        excerptText: "Synthetic visible output excerpt",
        excerptTruncated: false,
        activities: [{ class: "visible_text", timestampMs: NOW, toolName: null, status: "ready" }],
        reportAvailability: "available",
        reportText: "Synthetic exact handoff report. Receipt means delivery only.",
      },
    ],
  };
}

const FEED_HISTORY = "synthetic-history-v1";

function buildSyntheticFeed(scenario: FixtureScenario): {
  page: (beforeExchangeKey: string | null) => WidgetFeedPage;
  message: (eventKey: string) => WidgetFeedMessage | null;
} {
  const fullByKey = new Map<string, WidgetFeedMessage>();
  const items = feedExchanges(scenario, fullByKey);
  const eventCount = items.reduce(
    (total, exchange) => total + (exchange.request ? 1 : 0) + (exchange.completion ? 1 : 0),
    0,
  );
  return {
    page(beforeExchangeKey) {
      const visible = beforeExchangeKey ? [] : items;
      return {
        historyToken: FEED_HISTORY,
        items: visible,
        nextBeforeExchangeKey: null,
        hasEarlier: false,
        totalExchanges: items.length,
        totalEvents: eventCount,
        resetRequired: false,
      };
    },
    message(eventKey) {
      const full = fullByKey.get(eventKey);
      return full ? { ...full } : null;
    },
  };
}

function feedExchanges(
  scenario: FixtureScenario,
  fullByKey: Map<string, WidgetFeedMessage>,
): WidgetFeedExchange[] {
  if (
    scenario === "empty" ||
    scenario === "idle" ||
    scenario === "missing-selection" ||
    scenario === "ambiguous" ||
    scenario === "passive-fallback"
  ) {
    return [];
  }
  if (scenario === "live") {
    return [
      completedExchange(fullByKey, {
        index: 1,
        sessionKey: "session:synthetic-a",
        request: "Check the new conversation studio against the frozen design contract.",
        response: "The paper surfaces preserve exact content and the controls remain keyboard reachable.",
        contextOmitted: true,
        projection: "current-request",
      }),
      completedExchange(fullByKey, {
        index: 2,
        sessionKey: "session:synthetic-a",
        request: "Keep the chronology global.",
        response: "Session order stays quiet on the paper column.",
      }),
      completedExchange(fullByKey, {
        index: 3,
        sessionKey: "session:synthetic-b",
        request: "Start the next conversation.",
        response: "The divider stays free of paths and identifiers.",
      }),
    ];
  }
  if (scenario === "paused-unread" || scenario === "historical") {
    return [
      completedExchange(fullByKey, {
        index: 1,
        sessionKey: "session:synthetic-a",
        request: "Review the earlier exchange.",
        response: "The earlier exchange stays on the page.",
      }),
      completedExchange(fullByKey, {
        index: 2,
        sessionKey: "session:synthetic-b",
        request: "Note the messages that arrived while paused.",
        response: "Unread events stay counted until live resumes.",
      }),
    ];
  }
  if (scenario === "pending") {
    return [
      {
        exchangeKey: "exchange:synthetic-pending",
        sessionKey: "session:synthetic",
        timestampMs: NOW,
        request: feedMessage(fullByKey, {
          eventKey: "event:request",
          eventType: "request",
          speaker: "codex",
          recipient: "grok",
          timestampMs: NOW,
          status: "started",
          full: "Check the new conversation studio against the frozen design contract.",
        }),
        completion: null,
        pendingLabel: PENDING_LABEL,
      },
    ];
  }
  if (scenario === "error") {
    return [
      completedExchange(fullByKey, {
        index: 1,
        sessionKey: "session:synthetic",
        request: "Check the new conversation studio against the frozen design contract.",
        response: "Synthetic transport failure. No model speech was delivered.",
        responseType: "error",
        responseSpeaker: "codex",
      }),
    ];
  }
  if (scenario === "unknown-agent") {
    return [
      {
        exchangeKey: "exchange:synthetic-unknown",
        sessionKey: "session:synthetic",
        timestampMs: NOW,
        request: feedMessage(fullByKey, {
          eventKey: "event:request",
          eventType: "request",
          speaker: "nova",
          recipient: "codex",
          timestampMs: NOW,
          status: "ok",
          full: "Can the two chairs make room for a visiting reviewer?",
        }),
        completion: null,
        pendingLabel: null,
      },
    ];
  }
  if (scenario === "collapsed" || scenario === "expanded" || scenario === "maximum-exchange") {
    return [
      completedExchange(fullByKey, {
        index: 1,
        sessionKey: "session:synthetic",
        request: "Review the assembled evidence without rewriting any exact record.",
        response: `${"Exact synthetic record. ".repeat(250)}end`,
        truncateResponse: true,
      }),
    ];
  }
  if (scenario === "reversed-route") {
    return [
      completedExchange(fullByKey, {
        index: 1,
        sessionKey: "session:synthetic",
        request: "Check the new conversation studio against the frozen design contract.",
        response: "The paper surfaces preserve exact content and the controls remain keyboard reachable.",
        requestSpeaker: "grok",
        responseSpeaker: "codex",
        requestRecipient: "codex",
        responseRecipient: "grok",
      }),
    ];
  }
  return [
    completedExchange(fullByKey, {
      index: 1,
      sessionKey: "session:synthetic",
      request: "Check the new conversation studio against the frozen design contract.",
      response: "The paper surfaces preserve exact content and the controls remain keyboard reachable.",
    }),
  ];
}

function completedExchange(
  fullByKey: Map<string, WidgetFeedMessage>,
  input: {
    index: number;
    sessionKey: string;
    request: string;
    response: string;
    requestSpeaker?: string;
    responseSpeaker?: string;
    requestRecipient?: string;
    responseRecipient?: string;
    responseType?: "response" | "error";
    truncateResponse?: boolean;
    contextOmitted?: boolean;
    projection?: WidgetFeedMessage["projection"];
  },
): WidgetFeedExchange {
  const requestSpeaker = input.requestSpeaker ?? "codex";
  const responseSpeaker = input.responseSpeaker ?? (input.responseType === "error" ? "codex" : "grok");
  return {
    exchangeKey: `exchange:synthetic-${input.index}`,
    sessionKey: input.sessionKey,
    timestampMs: NOW + input.index * 1_000,
    request: feedMessage(fullByKey, {
      eventKey: `event:request-${input.index}`,
      eventType: "request",
      speaker: requestSpeaker,
      recipient: input.requestRecipient ?? "grok",
      timestampMs: NOW + input.index * 1_000,
      status: "ok",
      full: input.request,
      projection: input.projection,
      contextOmitted: input.contextOmitted,
    }),
    completion: feedMessage(fullByKey, {
      eventKey: `event:completion-${input.index}`,
      eventType: input.responseType ?? "response",
      speaker: responseSpeaker,
      recipient: input.responseRecipient ?? "codex",
      timestampMs: NOW + input.index * 1_000 + 42_000,
      status: input.responseType === "error" ? "failed" : "ok",
      full: input.response,
      truncate: input.truncateResponse,
    }),
    pendingLabel: null,
  };
}

function feedMessage(
  fullByKey: Map<string, WidgetFeedMessage>,
  input: {
    eventKey: string;
    eventType: WidgetFeedMessage["eventType"];
    speaker: string;
    recipient: string;
    timestampMs: number;
    status: string;
    full: string;
    truncate?: boolean;
    projection?: WidgetFeedMessage["projection"];
    contextOmitted?: boolean;
  },
): WidgetFeedMessage {
  const scalars = Array.from(input.full);
  const truncated = input.truncate === true && scalars.length > 4_000;
  const automatic: WidgetFeedMessage = {
    eventKey: input.eventKey,
    eventType: input.eventType,
    speaker: input.speaker,
    recipient: input.recipient,
    timestampMs: input.timestampMs,
    status: input.status,
    body: truncated ? scalars.slice(0, 4_000).join("") : input.full,
    fullCharacterLength: scalars.length,
    truncated,
    projection: input.projection ?? "exact",
    contextOmitted: input.contextOmitted === true,
  };
  fullByKey.set(input.eventKey, { ...automatic, body: input.full, truncated: false });
  return automatic;
}
