import { describe, expect, it } from "vitest";

import {
  CLOSED_CLASSES,
  DEFAULT_PEER_HEALTH_DIAGNOSTICS,
  DEFAULT_SETTINGS,
  INCIDENT_CLASSES,
  isIncidentClass,
  PEER_HEALTH_UNAVAILABLE_REASONS,
} from "./contracts";

describe("default widget settings", () => {
  it("uses the accepted top-right fresh-install geometry", () => {
    expect(DEFAULT_SETTINGS).toMatchObject({
      corner: "top-right",
      offsetX: 24,
      offsetY: 24,
      width: 560,
      height: 360,
    });
  });
});

describe("peer health contract tokens", () => {
  it("lists the closed classes and unavailable reasons from the health schema", () => {
    expect(CLOSED_CLASSES).toEqual([
      "usage_sample",
      "quota_exhausted",
      "capacity_throttle",
      "turn_error",
      "watchdog_killed",
      "mcp_stdout_undelivered",
      "handoff_unacknowledged",
    ]);
    expect(INCIDENT_CLASSES).toEqual(["quota_exhausted", "mcp_stdout_undelivered"]);
    expect(isIncidentClass("quota_exhausted")).toBe(true);
    expect(isIncidentClass("mcp_stdout_undelivered")).toBe(true);
    expect(isIncidentClass("capacity_throttle")).toBe(false);
    expect(isIncidentClass("turn_error")).toBe(false);
    expect(isIncidentClass("watchdog_killed")).toBe(false);
    expect(isIncidentClass("handoff_unacknowledged")).toBe(false);
    expect(PEER_HEALTH_UNAVAILABLE_REASONS).toEqual([
      "missing",
      "malformed",
      "locked",
      "arguments_not_allowed",
    ]);
    expect(DEFAULT_PEER_HEALTH_DIAGNOSTICS).toMatchObject({
      snapshotMissing: false,
      snapshotMalformed: false,
      snapshotLocked: false,
      journalIncompleteTrailing: false,
      malformedJournalLines: 0,
      oversizedJournalLines: 0,
      unsupportedJournalRecords: 0,
      quarantinedInbox: 0,
      malformedInbox: 0,
      soundFailures: 0,
      footerMissing: 0,
      codexSampleFailures: 0,
      lastCodexSampleFailureMs: null,
    });
  });
});
