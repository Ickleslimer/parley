import { describe, expect, it } from "vitest";

import type { ViewerStatus } from "../contracts";

import {
  completionStatusLabel,
  degradedBanner,
  exactPendingLabel,
  isParleyError,
  loadErrorLabel,
  PARLEY_ERROR_LABEL,
  PENDING_LABEL,
  pendingStatusLabel,
} from "./labels";

const status = (overrides: Partial<ViewerStatus> = {}): ViewerStatus => ({
  sourceState: "none",
  generation: 0,
  bytesRead: 0,
  sessionCount: 0,
  exchangeCount: 0,
  lastEventTimestampMs: null,
  trayAvailable: true,
  underlayState: "attached",
  widgetVisible: false,
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

describe("error and pending labels", () => {
  it("uses the exact pending phrase from the event engine", () => {
    expect(exactPendingLabel()).toBe("Request logged; no response event yet");
    expect(PENDING_LABEL).toBe("Request logged; no response event yet");
    expect(pendingStatusLabel(PENDING_LABEL)).toBe(PENDING_LABEL);
    expect(pendingStatusLabel(null)).toBeNull();
  });

  it("labels Parley execution errors without rewriting them", () => {
    expect(isParleyError({ eventType: "error" })).toBe(true);
    expect(isParleyError({ eventType: "response" })).toBe(false);
    expect(
      completionStatusLabel({
        completion: { eventType: "error" },
        pendingLabel: PENDING_LABEL,
      }),
    ).toBe(PARLEY_ERROR_LABEL);
    expect(
      completionStatusLabel({
        completion: { eventType: "response" },
        pendingLabel: PENDING_LABEL,
      }),
    ).toBe("Completion");
    expect(
      completionStatusLabel({
        completion: null,
        pendingLabel: PENDING_LABEL,
      }),
    ).toBe(PENDING_LABEL);
  });

  it("shows a prominent banner only for tray or underlay degradation", () => {
    expect(degradedBanner(null)).toBeNull();
    expect(degradedBanner(status())).toBeNull();
    expect(degradedBanner(status({ underlayState: "detached" }))).toBeNull();
    expect(degradedBanner(status({ underlayState: "attaching" }))).toBeNull();
    expect(degradedBanner(status({ trayAvailable: false }))).toBe(
      "System tray is unavailable. Exit remains available.",
    );
    expect(degradedBanner(status({ underlayState: "degraded" }))).toBe(
      "Desktop underlay is degraded. The widget may be hidden.",
    );
    expect(degradedBanner(status({ trayAvailable: false, underlayState: "degraded" }))).toBe(
      "System tray and desktop underlay are degraded. The widget may be hidden. Exit remains available.",
    );
  });

  it("keeps load failures as plain action labels", () => {
    expect(loadErrorLabel("load sessions")).toBe("Unable to load sessions");
  });
});
