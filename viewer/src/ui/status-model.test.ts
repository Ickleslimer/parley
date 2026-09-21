import { describe, expect, it } from "vitest";

import type { MessagePreview, ViewerStatus, WidgetSnapshot } from "../contracts";

import { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";
import { widgetModel } from "./status-model";

const preview = (overrides: Partial<MessagePreview> = {}): MessagePreview => ({
  eventKey: "key-req-1",
  eventId: "req-1",
  eventType: "request",
  speaker: "codex",
  recipient: "grok",
  timestampMs: 0,
  status: "started",
  excerpt: " Ship the engine",
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
      path: "C:\\\\events.jsonl",
      identity: "c:\\\\events.jsonl",
      sourceState: "watching",
      generation: 1,
      bytesRead: 100,
      sessionCount: 1,
      exchangeCount: 1,
      lastEventTimestampMs: 0,
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
});

describe("widget model", () => {
  it("composes exact excerpts, extracted-task labeling, and pending text", () => {
    const snapshot: WidgetSnapshot = {
      sessionKey: "key-s-1",
      exchangeKey: "key-ex-1",
      sessionId: "s-1",
      exchangeId: "ex-1",
      request: preview(),
      completion: null,
      pendingLabel: PENDING_LABEL,
    };
    const model = widgetModel({ status: status(), snapshot, loadError: null, utc: true });
    expect(model.idleLabel).toBeNull();
    expect(model.request?.excerpt).toBe(" Ship the engine");
    expect(model.request?.extractedLabel).toBe(EXTRACTED_TASK_LABEL);
    expect(model.pendingLabel).toBe(PENDING_LABEL);
    expect(model.completion).toBeNull();
    expect(model.banner).toBeNull();
  });

  it("surfaces Parley errors, idle source labels, and tray degradation", () => {
    const snapshot: WidgetSnapshot = {
      sessionKey: "key-s-1",
      exchangeKey: "key-ex-2",
      sessionId: "s-1",
      exchangeId: "ex-2",
      request: preview({ excerpt: "second", excerptExtracted: false }),
      completion: preview({
        eventId: "err-2",
        eventType: "error",
        excerpt: "boom",
        excerptExtracted: false,
        status: "failed",
      }),
      pendingLabel: null,
    };
    const model = widgetModel({
      status: status({ trayAvailable: false, underlayState: "degraded" }),
      snapshot,
      loadError: null,
      utc: true,
    });
    expect(model.completion?.heading).toBe(PARLEY_ERROR_LABEL);
    expect(model.completion?.excerpt).toBe("boom");
    expect(model.banner).toContain("tray");
    expect(model.banner).toContain("underlay");

    const idle = widgetModel({
      status: status({ sourceState: "missing" }),
      snapshot: {
        sessionKey: null,
        exchangeKey: null,
        sessionId: null,
        exchangeId: null,
        request: null,
        completion: null,
        pendingLabel: null,
      },
      loadError: null,
    });
    expect(idle.idleLabel).toBe("Event log is missing");
    expect(idle.request).toBeNull();

    const connecting = widgetModel({ status: null, snapshot: null, loadError: null });
    expect(connecting.idleLabel).toBe("Connecting\u2026");
    expect(connecting.sourceLabel).toBe("Connecting\u2026");
    expect("content" in connecting).toBe(false);

    const failed = widgetModel({
      status: null,
      snapshot: null,
      loadError: "Unable to load widget snapshot",
    });
    expect(failed.idleLabel).toBeNull();
    expect(failed.loadError).toBe("Unable to load widget snapshot");
  });
});
