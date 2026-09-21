import { describe, expect, it } from "vitest";

import type {
  EventContent,
  HandoffSelection,
  PeerHealthSnapshot,
  PeerIncident,
} from "../contracts";
import { DEFAULT_PEER_HEALTH_DIAGNOSTICS } from "../contracts";

import {
  applyOpenHandoff,
  canAcknowledgeIncident,
  CURRENT_SNAPSHOT_LABEL,
  EXACT_PRESERVED_RESPONSE_LABEL,
  formatCodexSample,
  formatGrokObservation,
  formatPeerHealthDiagnostics,
  handoffSemanticsLabel,
  LOADING_PEER_HEALTH_LABEL,
  NO_CODEX_SAMPLE_LABEL,
  NO_GROK_OBSERVATION_LABEL,
  NO_SNAPSHOT_YET_LABEL,
  OPEN_LATEST_HANDOFF_LABEL,
  PEER_HEALTH_CAPTION,
  peerHealthActions,
  presentPeerHealth,
  presentedHandoffLabel,
  QUOTA_HANDOFF_LABEL,
  SILENCE_IS_NOT_FAILURE,
  snapshotAvailabilityLabel,
  STALE_SNAPSHOT_LABEL,
  TEST_CHIME_LABEL,
} from "./peer-health";

const event = (overrides: Partial<EventContent> = {}): EventContent => ({
  eventId: "evt-1",
  exchangeId: "ex-1",
  sessionId: "ses-1",
  eventType: "response",
  speaker: "grok",
  recipient: "codex",
  timestampMs: 1_704_067_200_000,
  status: "ok",
  durationMs: 12,
  error: null,
  content: "exact preserved Grok stdout",
  ...overrides,
});

const incident = (overrides: Partial<PeerIncident> = {}): PeerIncident => ({
  incidentId: "inc-1",
  class: "quota_exhausted",
  source: "codex",
  status: "active",
  openedMs: 1_704_067_200_000,
  asOfMs: 1_704_067_200_000,
  recoveredMs: null,
  acknowledged: false,
  sessionId: "ses-1",
  eventId: "evt-1",
  exchangeId: "ex-1",
  ...overrides,
});

const snapshot = (overrides: Partial<PeerHealthSnapshot> = {}): PeerHealthSnapshot => ({
  schemaVersion: 1,
  generatedMs: 1_704_067_260_000,
  asOfMs: 1_704_067_200_000,
  muted: false,
  unreadCount: 0,
  latestCodexSample: null,
  latestGrokObservation: null,
  activeIncidents: [],
  recentIncidents: [],
  unavailable: null,
  stale: false,
  diagnostics: { ...DEFAULT_PEER_HEALTH_DIAGNOSTICS },
  ...overrides,
});

describe("peer health snapshot state", () => {
  it("labels stale and unavailable snapshots without treating silence as failure", () => {
    const stale = snapshot({ stale: true });
    expect(snapshotAvailabilityLabel(stale)).toBe(STALE_SNAPSHOT_LABEL);
    const missing = snapshot({
      stale: true,
      unavailable: { reason: "missing" },
      latestCodexSample: null,
      latestGrokObservation: null,
    });
    const view = presentPeerHealth({
      snapshot: missing,
      error: null,
      loading: false,
      busy: false,
      handoffLabel: null,
    });
    expect(view.availability).toBe("Peer health unavailable (missing). Snapshot is stale.");
    expect(view.availabilityKind).toBe("unavailable");
    expect(view.silenceNote).toBe(SILENCE_IS_NOT_FAILURE);
    expect(view.codexLabel).toBe(NO_CODEX_SAMPLE_LABEL);
    expect(view.grokLabel).toBe(NO_GROK_OBSERVATION_LABEL);
    expect(view.codexLabel).not.toMatch(/Codex is (alive|down|dead)/i);
    expect(view.grokLabel).not.toMatch(/Grok is (alive|down|dead)/i);
    expect(view.caption).toBe(PEER_HEALTH_CAPTION);
    expect(view.caption).toMatch(/not inferred/i);
  });

  it("keeps unavailable reasons as exact tokens and does not invent peer-alive", () => {
    expect(snapshotAvailabilityLabel(snapshot({ unavailable: { reason: "malformed" } }))).toBe(
      "Peer health unavailable (malformed).",
    );
    expect(
      snapshotAvailabilityLabel(snapshot({ stale: true, unavailable: { reason: "locked" } })),
    ).toBe("Peer health unavailable (locked). Snapshot is stale.");
    expect(
      snapshotAvailabilityLabel(
        snapshot({ stale: true, unavailable: { reason: "arguments_not_allowed" } }),
      ),
    ).toContain("arguments_not_allowed");
    expect(snapshotAvailabilityLabel(snapshot())).toBe(CURRENT_SNAPSHOT_LABEL);
  });

  it("renders asymmetric Codex and Grok evidence without synthesizing liveness", () => {
    const view = presentPeerHealth({
      snapshot: snapshot({
        latestCodexSample: {
          usedPercent: 42.5,
          resetsAt: "2026-01-01T00:00:00Z",
          planType: "plus",
          rateLimitReachedType: "primary",
          asOfMs: 1_704_067_200_000,
        },
        latestGrokObservation: {
          class: "capacity_throttle",
          asOfMs: 1_704_067_200_000,
          success: false,
          httpStatus: 429,
          providerCode: "resource_exhausted",
        },
      }),
      error: null,
      loading: false,
      busy: false,
      handoffLabel: null,
    });
    expect(view.codexLabel).toContain("42.5% used");
    expect(view.codexLabel).toContain("plan plus");
    expect(view.codexLabel).toContain("resets 2026-01-01T00:00:00Z");
    expect(view.codexLabel).toContain("rate limit primary");
    expect(view.grokLabel).toContain("capacity_throttle");
    expect(view.grokLabel).toContain("HTTP 429");
    expect(view.grokLabel).toContain("code resource_exhausted");
    expect(view.grokLabel).toContain("unsuccessful");
    expect(view.codexLabel).not.toMatch(/peer[- ]?(alive|down)/i);
    expect(view.grokLabel).not.toMatch(/peer[- ]?(alive|down)/i);
  });

  it("shows loading without a snapshot instead of a fabricated health state", () => {
    const view = presentPeerHealth({
      snapshot: null,
      error: null,
      loading: true,
      busy: false,
      handoffLabel: null,
    });
    expect(view.availability).toBe(LOADING_PEER_HEALTH_LABEL);
    expect(view.availabilityKind).toBe("loading");
    expect(view.silenceNote).toBeNull();
    expect(view.actions.muteEnabled).toBe(false);
    expect(view.actions.testChimeEnabled).toBe(false);
    expect(view.actions.openHandoffEnabled).toBe(false);
  });

  it("shows a load error without fabricating a snapshot or a liveness flag", () => {
    const view = presentPeerHealth({
      snapshot: null,
      error: "Unable to load peer health",
      loading: false,
      busy: false,
      handoffLabel: null,
    });
    expect(view.availabilityKind).toBe("error");
    expect(view.availability).toBe(NO_SNAPSHOT_YET_LABEL);
    expect(view.error).toBe("Unable to load peer health");
    expect(view.codexLabel).toBe(NO_SNAPSHOT_YET_LABEL);
    expect(view.grokLabel).toBe(NO_SNAPSHOT_YET_LABEL);
    expect(view.codexLabel).not.toMatch(/alive|down/i);
  });
});

describe("peer health labels", () => {
  it("keeps missing samples as absence, not failure from silence", () => {
    expect(formatCodexSample(null)).toBe(NO_CODEX_SAMPLE_LABEL);
    expect(formatGrokObservation(null)).toBe(NO_GROK_OBSERVATION_LABEL);
    expect(formatCodexSample(null)).toMatch(/not a Codex-down signal/);
    expect(formatGrokObservation(null)).toMatch(/not a Grok-down signal/);
  });

  it("labels mcp_stdout_undelivered as exact preserved response and quota as not proven undelivered", () => {
    expect(handoffSemanticsLabel("mcp_stdout_undelivered")).toBe(EXACT_PRESERVED_RESPONSE_LABEL);
    expect(handoffSemanticsLabel("quota_exhausted")).toBe(QUOTA_HANDOFF_LABEL);
    expect(handoffSemanticsLabel("quota_exhausted")).toMatch(/not proven undelivered/);
    expect(handoffSemanticsLabel("quota_exhausted")).toMatch(/latest preceding Grok reply/i);
    expect(handoffSemanticsLabel("turn_error")).toBeNull();
    expect(handoffSemanticsLabel("watchdog_killed")).toBeNull();
    expect(handoffSemanticsLabel("capacity_throttle")).toBeNull();
    expect(handoffSemanticsLabel("usage_sample")).toBeNull();
    expect(handoffSemanticsLabel("quota_exhausted", true)).toBe(EXACT_PRESERVED_RESPONSE_LABEL);
  });

  it("renders diagnostics compactly including schema flags and counts", () => {
    const label = formatPeerHealthDiagnostics({
      ...DEFAULT_PEER_HEALTH_DIAGNOSTICS,
      snapshotMissing: true,
      journalIncompleteTrailing: true,
      malformedJournalLines: 2,
      footerMissing: 1,
      lastCodexSampleFailureMs: 0,
    });
    expect(label).toContain("snapshot missing");
    expect(label).toContain("journal incomplete trailing");
    expect(label).toContain("malformed journal 2");
    expect(label).toContain("footer missing 1");
    expect(label).toContain("last Codex sample failure");
  });
});

describe("peer health actions", () => {
  it("enables mute, test chime, and open-handoff only for an idle loaded snapshot", () => {
    const idle = peerHealthActions({ snapshot: snapshot({ muted: true }), busy: false });
    expect(idle.muteEnabled).toBe(true);
    expect(idle.muteChecked).toBe(true);
    expect(idle.nextMuted).toBe(false);
    expect(idle.testChimeEnabled).toBe(true);
    expect(idle.openHandoffEnabled).toBe(true);
    expect(idle.testChimeLabel).toBe(TEST_CHIME_LABEL);
    expect(idle.openHandoffLabel).toBe(OPEN_LATEST_HANDOFF_LABEL);

    const busy = peerHealthActions({ snapshot: snapshot(), busy: true });
    expect(busy.muteEnabled).toBe(false);
    expect(busy.testChimeEnabled).toBe(false);
    expect(busy.openHandoffEnabled).toBe(false);

    const missing = peerHealthActions({ snapshot: null, busy: false });
    expect(missing.muteEnabled).toBe(false);
    expect(missing.testChimeEnabled).toBe(false);
    expect(missing.openHandoffEnabled).toBe(false);
  });

  it("allows acknowledgement only for unacknowledged incidents while idle", () => {
    const open = incident({ acknowledged: false });
    const acked = incident({ incidentId: "inc-2", acknowledged: true });
    expect(canAcknowledgeIncident(open, false)).toBe(true);
    expect(canAcknowledgeIncident(open, true)).toBe(false);
    expect(canAcknowledgeIncident(acked, false)).toBe(false);
    const view = presentPeerHealth({
      snapshot: snapshot({
        unreadCount: 1,
        activeIncidents: [open],
        recentIncidents: [acked],
      }),
      error: null,
      loading: false,
      busy: false,
      handoffLabel: null,
    });
    expect(view.unreadLabel).toBe("1 unacknowledged active incident");
    expect(view.activeIncidents[0]?.canAcknowledge).toBe(true);
    expect(view.recentIncidents[0]?.canAcknowledge).toBe(false);
    expect(view.activeIncidents[0]?.handoffHint).toBe(QUOTA_HANDOFF_LABEL);
    const busyView = presentPeerHealth({
      snapshot: snapshot({ activeIncidents: [open] }),
      error: null,
      loading: false,
      busy: true,
      handoffLabel: null,
    });
    expect(busyView.activeIncidents[0]?.canAcknowledge).toBe(false);
    expect(busyView.actions.muteEnabled).toBe(false);
  });

  it("applies Open Latest Handoff by replacing the event pane only when an event is returned", () => {
    const exact = event();
    const undelivered: HandoffSelection = {
      event: exact,
      label: "Latest undelivered Grok stdout",
      incidentId: "inc-u",
      exactUndelivered: true,
    };
    const withIncident = snapshot({
      recentIncidents: [
        incident({
          incidentId: "inc-u",
          class: "mcp_stdout_undelivered",
          source: "parley",
        }),
      ],
    });
    const applied = applyOpenHandoff(undelivered, withIncident);
    expect(applied.replaceEvent).toBe(true);
    expect(applied.event).toEqual(exact);
    expect(applied.event?.content).toBe("exact preserved Grok stdout");
    expect(applied.label).toContain("Latest undelivered Grok stdout");
    expect(applied.label).toContain(EXACT_PRESERVED_RESPONSE_LABEL);

    const quota: HandoffSelection = {
      event: null,
      label: "Latest preceding Grok reply",
      incidentId: "inc-1",
      exactUndelivered: false,
    };
    const quotaApplied = applyOpenHandoff(
      quota,
      snapshot({ activeIncidents: [incident({ class: "quota_exhausted" })] }),
    );
    expect(quotaApplied.replaceEvent).toBe(false);
    expect(quotaApplied.event).toBeNull();
    expect(quotaApplied.label).toContain("Latest preceding Grok reply");
    expect(quotaApplied.label).toContain(QUOTA_HANDOFF_LABEL);
    expect(presentedHandoffLabel(quota, snapshot({ activeIncidents: [incident()] }))).toContain(
      "not proven undelivered",
    );
    expect(presentedHandoffLabel(quota, snapshot({ activeIncidents: [incident()] }))).not.toContain(
      EXACT_PRESERVED_RESPONSE_LABEL,
    );
  });

  it("keeps a returned handoff label when a test chime is requested", () => {
    const view = presentPeerHealth({
      snapshot: snapshot(),
      error: null,
      loading: false,
      busy: false,
      handoffLabel: "Latest preceding Grok reply",
      chimeStatus: "Test chime requested",
    });
    expect(view.handoffLabel).toContain("Latest preceding Grok reply");
    expect(view.handoffLabel).toContain("Test chime requested");
  });

  it("surfaces quota handoff as latest context, not a proven undelivered response", () => {
    const view = presentPeerHealth({
      snapshot: snapshot({
        activeIncidents: [incident({ class: "quota_exhausted" })],
        recentIncidents: [
          incident({
            incidentId: "inc-u",
            class: "mcp_stdout_undelivered",
            source: "parley",
            acknowledged: true,
          }),
        ],
      }),
      error: null,
      loading: false,
      busy: false,
      handoffLabel: "Latest preceding Grok reply \u00b7 " + QUOTA_HANDOFF_LABEL,
    });
    expect(view.activeIncidents[0]?.handoffHint).toBe(QUOTA_HANDOFF_LABEL);
    expect(view.recentIncidents[0]?.handoffHint).toBe(EXACT_PRESERVED_RESPONSE_LABEL);
    expect(view.handoffLabel).toContain("not proven undelivered");
  });
});
