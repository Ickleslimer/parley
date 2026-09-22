import { describe, expect, it } from "vitest";

import type {
  PeerActivityDiagnostics,
  PeerActivitySnapshot,
  PeerHandoffItem,
} from "../contracts";
import {
  freshnessLabel,
  peerActivityRevision,
  PEER_ACTIVITY_CAPTION,
  presentHandoff,
  presentPeerActivity,
} from "./peer-activity";

const diagnostics = (): PeerActivityDiagnostics => ({
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
});

const handoff = (overrides: Partial<PeerHandoffItem> = {}): PeerHandoffItem => ({
  jobId: "11111111-1111-4111-8111-111111111111",
  handoffId: "22222222-2222-4222-8222-222222222222",
  sourceSessionId: "codex-session",
  targetSessionId: "grok-session",
  state: "awaiting_ack",
  phase: "awaiting_ack",
  processState: "alive",
  createdAtMs: 1_000,
  updatedAtMs: 2_000,
  lastActivityMs: 1_900,
  readyAtMs: 1_800,
  deadlineMs: 21_601_800,
  receiptAtMs: null,
  alertIncidentId: null,
  recordDiagnostic: null,
  excerptText: "exact visible output",
  excerptTruncated: false,
  activities: [
    { class: "thought", timestampMs: 1_500, toolName: null, status: null },
    { class: "tool_call", timestampMs: 1_600, toolName: "read_file", status: "completed" },
  ],
  reportAvailability: "available",
  reportText: "exact durable report",
  ...overrides,
});

const snapshot = (overrides: Partial<PeerActivitySnapshot> = {}): PeerActivitySnapshot => ({
  generatedMs: 2_000,
  assessment: "not_inferred",
  source: { kind: "local_app_data" },
  unavailable: null,
  diagnostics: diagnostics(),
  shownCount: 1,
  totalCount: 1,
  truncated: false,
  handoffs: [handoff()],
  ...overrides,
});

describe("peer activity presentation", () => {
  it("shows exact visible output and reports without inventing a peer verdict", () => {
    const view = presentPeerActivity({
      snapshot: snapshot(),
      error: null,
      loading: false,
    });
    expect(view.caption).toBe(PEER_ACTIVITY_CAPTION);
    expect(view.summary).toContain("assessment not_inferred");
    expect(view.handoffs[0]?.excerpt).toBe("exact visible output");
    expect(view.handoffs[0]?.report).toBe("exact durable report");
    expect(view.handoffs[0]?.activities[0]?.label).toBe("thought");
    expect(JSON.stringify(view)).not.toMatch(/reasoning text|raw input|raw output/i);
    expect(JSON.stringify(view)).not.toMatch(/peer (is )?(alive|down|dead)/i);
  });

  it("describes receipt as delivery only and leaves pending jobs unacknowledged", () => {
    const pending = presentHandoff(handoff(), 2_000);
    expect(pending.receipt).toBe("No Codex receipt recorded.");
    const received = presentHandoff(handoff({ receiptAtMs: 1_950, state: "acknowledged" }), 2_000);
    expect(received.receipt).toContain("delivery only, not acceptance");
  });

  it("exposes explicit missing evidence and never converts silence into failure", () => {
    const view = presentPeerActivity({
      snapshot: snapshot({
        unavailable: "locked",
        shownCount: 0,
        totalCount: 0,
        handoffs: [],
      }),
      error: null,
      loading: false,
    });
    expect(view.availability).toContain("unavailable (locked)");
    expect(view.empty).not.toBeNull();
    expect(freshnessLabel(2_000, null)).toContain("no peer state is inferred");
  });

  it("never exposes a mismatched report as exact content", () => {
    const presented = presentHandoff(
      handoff({ reportAvailability: "malformed", reportText: null }),
      2_000,
    );
    expect(presented.report).toBeNull();
    expect(presented.reportStatus).toBe("Report malformed");
  });

  it("keeps the render revision stable across read-only poll timestamps", () => {
    const first = snapshot({ generatedMs: 2_000 });
    const later = snapshot({ generatedMs: 3_000 });
    expect(peerActivityRevision(first)).toBe(peerActivityRevision(later));
    later.handoffs[0] = handoff({ receiptAtMs: 2_500, state: "acknowledged" });
    expect(peerActivityRevision(first)).not.toBe(peerActivityRevision(later));
  });
});
