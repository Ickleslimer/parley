/**
 * @vitest-environment happy-dom
 */
import { describe, expect, it } from "vitest";

import type { PeerActivitySnapshot, PeerHealthSnapshot, PeerHandoffItem, PeerIncident } from "../contracts";
import { DEFAULT_PEER_HEALTH_DIAGNOSTICS } from "../contracts";

import {
  activityAttentionLabel,
  buildInspector,
  pendingHandoffCount,
  unreadIncidentCount,
} from "./inspector-tabs";

const incident = (overrides: Partial<PeerIncident> = {}): PeerIncident => ({
  incidentId: "incident-1",
  class: "quota_exhausted",
  source: "codex",
  status: "active",
  openedMs: 1,
  asOfMs: 1,
  recoveredMs: null,
  acknowledged: false,
  sessionId: null,
  eventId: null,
  exchangeId: null,
  ...overrides,
});

const handoff = (overrides: Partial<PeerHandoffItem> = {}): PeerHandoffItem => ({
  jobId: "job-1",
  handoffId: "handoff-1",
  sourceSessionId: null,
  targetSessionId: null,
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
  excerptText: null,
  excerptTruncated: false,
  activities: [],
  reportAvailability: "absent",
  reportText: null,
  ...overrides,
});

describe("inspector attention", () => {
  it("counts unread incidents and unreceived handoffs without treating diagnostics as incidents", () => {
    const health: PeerHealthSnapshot = {
      schemaVersion: 3,
      generatedMs: 1,
      asOfMs: 1,
      muted: false,
      unreadCount: 9,
      latestCodexSample: null,
      latestGrokObservation: null,
      activeIncidents: [
        incident(),
        incident({ incidentId: "incident-2", class: "turn_error" }),
        incident({ incidentId: "incident-3", acknowledged: true }),
      ],
      recentIncidents: [],
      unavailable: null,
      stale: false,
      diagnostics: { ...DEFAULT_PEER_HEALTH_DIAGNOSTICS },
    };
    const activity: PeerActivitySnapshot = {
      generatedMs: 1,
      assessment: "not_inferred",
      source: { kind: "unconfigured" },
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
      shownCount: 2,
      totalCount: 2,
      truncated: false,
      handoffs: [handoff(), handoff({ jobId: "job-2", state: "acknowledged", receiptAtMs: 5 })],
    };
    expect(unreadIncidentCount(health)).toBe(1);
    expect(pendingHandoffCount(activity)).toBe(1);
    expect(activityAttentionLabel(1, 1)).toBe("1 unread, 1 pending");
  });
});

describe("manual inspector tabs", () => {
  it("moves focus with arrows and activates only on enter, space, or click", () => {
    const inspector = buildInspector();
    document.body.append(inspector.region);
    expect(inspector.selected()).toBe("event");
    expect(inspector.panels.activity.hidden).toBe(true);

    inspector.tabs.event.focus();
    inspector.tabs.event.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(document.activeElement).toBe(inspector.tabs.activity);
    expect(inspector.selected()).toBe("event");
    expect(inspector.panels.event.hidden).toBe(false);
    expect(inspector.panels.activity.hidden).toBe(true);

    inspector.tabs.activity.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(inspector.selected()).toBe("activity");
    expect(inspector.panels.activity.hidden).toBe(false);

    inspector.activate("event");
    inspector.setActivityAttention("2 unread, 1 pending");
    expect(inspector.selected()).toBe("event");
    expect(inspector.activityCount.textContent).toBe("2 unread, 1 pending");
    expect(inspector.tabs.activity.getAttribute("aria-selected")).toBe("false");

    inspector.tabs.sources.click();
    expect(inspector.selected()).toBe("sources");
    expect(inspector.panels.sources.hidden).toBe(false);
  });
});
