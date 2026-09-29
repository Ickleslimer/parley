import { describe, expect, it } from "vitest";

import type { Diagnostics, ViewerStatus } from "../contracts";

import {
  formatCharacterCount,
  formatCount,
  formatDiagnostics,
  formatDuration,
  formatEventType,
  formatRoute,
  formatRuntimeHealth,
  formatSourceLine,
  formatSourceState,
  formatTimestamp,
  idleWidgetLabel,
  PARLEY_ERROR_LABEL,
} from "./format";

const diagnostics = (overrides: Partial<Diagnostics> = {}): Diagnostics => ({
  malformedLines: 0,
  oversizedLines: 0,
  unsupportedRecords: 0,
  duplicateEvents: 0,
  ioErrors: 0,
  aliasCollisions: 0,
  lastError: null,
  ...overrides,
});

const status = (overrides: Partial<ViewerStatus> = {}): ViewerStatus => ({
  sourceState: "watching",
  generation: 1,
  bytesRead: 2048,
  sessionCount: 2,
  exchangeCount: 5,
  lastEventTimestampMs: 1,
  trayAvailable: true,
  underlayState: "attached",
  desktopRuntimeState: "interactive",
  desktopFallbackReason: null,
  widgetVisible: true,
  diagnostics: diagnostics(),
  sources: [
    {
      path: "C:\\\\logs\\\\events.jsonl",
      identity: "c:\\\\logs\\\\events.jsonl",
      sourceState: "watching",
      generation: 1,
      bytesRead: 2048,
      sessionCount: 2,
      exchangeCount: 5,
      lastEventTimestampMs: 1,
      diagnostics: diagnostics(),
      aliasOf: null,
    },
  ],
  ...overrides,
});

describe("formatting", () => {
  it("formats timestamps as exact UTC fields without summarizing", () => {
    expect(formatTimestamp(null, true)).toBe("No timestamp");
    expect(formatTimestamp(Number.NaN, true)).toBe("No timestamp");
    expect(formatTimestamp(0, true)).toBe("1970-01-01 00:00:00 UTC");
    expect(formatTimestamp(1_704_067_200_000, true)).toBe("2024-01-01 00:00:00 UTC");
  });

  it("formats routes and event types as plain labels", () => {
    expect(formatRoute("codex", "grok")).toBe("codex \u2192 grok");
    expect(formatEventType("request")).toBe("Request");
    expect(formatEventType("response")).toBe("Response");
    expect(formatEventType("error")).toBe(PARLEY_ERROR_LABEL);
  });

  it("formats source, idle, count, duration, and diagnostics labels", () => {
    expect(formatSourceState("none")).toBe("No event log selected");
    expect(formatSourceState("missing")).toBe("Event log is missing");
    expect(formatSourceState("watching")).toBe("Watching");
    expect(formatSourceState("degraded")).toBe("Source degraded");
    expect(idleWidgetLabel(null)).toBe("No event log selected");
    expect(idleWidgetLabel("watching")).toBe("Waiting for conversation events");
    expect(formatCount(1, "session")).toBe("1 session");
    expect(formatCount(3, "session")).toBe("3 sessions");
    expect(formatCharacterCount(60001)).toBe("60001 characters");
    expect(formatDuration(null)).toBe("No duration");
    expect(formatDuration(9)).toBe("9 ms");
    expect(formatDiagnostics(diagnostics({ malformedLines: 2, lastError: "disk" }))).toBe(
      "malformed 2; oversized 0; unsupported 0; duplicates 0; I/O 0; aliases 0; last error: disk",
    );
  });

  it("formats aggregate source counts and runtime health", () => {
    const watching = status();
    expect(formatSourceLine(watching)).toContain("1 source");
    expect(formatSourceLine(watching)).not.toContain("sessions");
    expect(formatSourceLine(watching)).toContain("5 exchanges");
    expect(formatRuntimeHealth(watching)).toBe(
      "Tray available. Underlay attached; Widget visible",
    );
    expect(
      formatRuntimeHealth(
        status({ trayAvailable: false, underlayState: "degraded", widgetVisible: false }),
      ),
    ).toBe("Tray unavailable. Underlay degraded; Widget hidden");
  });
});
