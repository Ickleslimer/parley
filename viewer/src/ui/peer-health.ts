import type {
  ClosedClass,
  CodexSample,
  GrokObservation,
  HandoffSelection,
  PeerHealthDiagnostics,
  PeerHealthSnapshot,
  PeerHealthSource,
  PeerHealthUnavailableReason,
  PeerIncident,
  PeerIncidentStatus,
} from "../contracts";

import { button, el, labelledControl, setText } from "./dom";
import { formatCount, formatTimestamp } from "./format";

export const PEER_HEALTH_TITLE = "Peer Health";
export const PEER_HEALTH_CAPTION =
  "Read-only Codex and Grok evidence for the Two Chairs collaboration. Peer-alive and peer-down are not inferred.";
export const SILENCE_IS_NOT_FAILURE = "Silence is not evidence of failure.";
export const NO_CODEX_SAMPLE_LABEL =
  "No Codex usage sample. Absence is not a Codex-down signal.";
export const NO_GROK_OBSERVATION_LABEL =
  "No Grok observation. Absence is not a Grok-down signal.";
export const EXACT_PRESERVED_RESPONSE_LABEL = "Exact preserved response";
export const QUOTA_HANDOFF_LABEL =
  "Latest preceding Grok reply / latest context, not proven undelivered";
export const STALE_SNAPSHOT_LABEL = "Peer health snapshot is stale.";
export const CURRENT_SNAPSHOT_LABEL = "Peer health snapshot is current.";
export const LOADING_PEER_HEALTH_LABEL = "Loading peer health\u2026";
export const NO_SNAPSHOT_YET_LABEL = "No peer-health snapshot yet.";
export const NO_ACTIVE_INCIDENTS_LABEL = "No active incidents";
export const NO_RECENT_INCIDENTS_LABEL = "No recent incidents";
export const ACKNOWLEDGE_LABEL = "Acknowledge";
export const MUTE_CONTROL_LABEL = "Mute peer-health chime";
export const TEST_CHIME_LABEL = "Test chime";
export const TEST_CHIME_REQUESTED_LABEL = "Test chime requested";
export const OPEN_LATEST_HANDOFF_LABEL = "Open Latest Handoff";
export const CHIME_MUTED_LABEL = "Peer-health chime is muted";
export const CHIME_AUDIBLE_LABEL = "Peer-health chime is audible";

export type PeerHealthAvailabilityKind =
  | "loading"
  | "error"
  | "unavailable"
  | "stale"
  | "current";

export interface PresentedPeerIncident {
  incidentId: string;
  title: string;
  meta: string;
  class: ClosedClass;
  source: PeerHealthSource;
  status: PeerIncidentStatus;
  acknowledged: boolean;
  canAcknowledge: boolean;
  handoffHint: string | null;
}

export interface PeerHealthActionModel {
  muteLabel: string;
  muteChecked: boolean;
  muteEnabled: boolean;
  nextMuted: boolean;
  testChimeLabel: string;
  testChimeEnabled: boolean;
  openHandoffLabel: string;
  openHandoffEnabled: boolean;
}

export interface PresentedPeerHealth {
  title: string;
  caption: string;
  availability: string;
  availabilityKind: PeerHealthAvailabilityKind;
  silenceNote: string | null;
  mutedLabel: string;
  unreadLabel: string;
  generatedLabel: string;
  codexLabel: string;
  grokLabel: string;
  diagnosticsLabel: string;
  activeHeading: string;
  recentHeading: string;
  activeEmpty: string | null;
  recentEmpty: string | null;
  activeIncidents: PresentedPeerIncident[];
  recentIncidents: PresentedPeerIncident[];
  actions: PeerHealthActionModel;
  handoffLabel: string | null;
  error: string | null;
}

export interface PeerHealthUiState {
  snapshot: PeerHealthSnapshot | null;
  error: string | null;
  loading: boolean;
  busy: boolean;
  handoffLabel: string | null;
  chimeStatus?: string | null;
}

export interface AppliedHandoff {
  event: HandoffSelection["event"];
  replaceEvent: boolean;
  label: string;
}

export interface PeerHealthNodes {
  section: HTMLElement;
  caption: HTMLParagraphElement;
  availability: HTMLParagraphElement;
  silenceNote: HTMLParagraphElement;
  errorLine: HTMLParagraphElement;
  mutedLine: HTMLParagraphElement;
  unreadLine: HTMLParagraphElement;
  generatedLine: HTMLParagraphElement;
  codexLine: HTMLParagraphElement;
  grokLine: HTMLParagraphElement;
  diagnosticsLine: HTMLParagraphElement;
  activeEmpty: HTMLParagraphElement;
  activeList: HTMLUListElement;
  recentEmpty: HTMLParagraphElement;
  recentList: HTMLUListElement;
  handoffLine: HTMLParagraphElement;
  muted: HTMLInputElement;
  testChime: HTMLButtonElement;
  openHandoff: HTMLButtonElement;
}

export function unavailableReasonLabel(reason: PeerHealthUnavailableReason): string {
  switch (reason) {
    case "missing":
    case "malformed":
    case "locked":
    case "arguments_not_allowed":
      return reason;
  }
}

export function snapshotAvailabilityLabel(snapshot: PeerHealthSnapshot): string {
  if (snapshot.unavailable) {
    const reason = unavailableReasonLabel(snapshot.unavailable.reason);
    if (snapshot.stale) {
      return `Peer health unavailable (${reason}). Snapshot is stale.`;
    }
    return `Peer health unavailable (${reason}).`;
  }
  if (snapshot.stale) {
    return STALE_SNAPSHOT_LABEL;
  }
  return CURRENT_SNAPSHOT_LABEL;
}

export function silenceNoteForSnapshot(snapshot: PeerHealthSnapshot): string | null {
  if (snapshot.unavailable || snapshot.stale) {
    return SILENCE_IS_NOT_FAILURE;
  }
  return null;
}

export function closedClassLabel(closedClass: ClosedClass): string {
  switch (closedClass) {
    case "usage_sample":
      return "usage_sample \u00b7 Usage sample";
    case "quota_exhausted":
      return "quota_exhausted \u00b7 Quota exhausted";
    case "capacity_throttle":
      return "capacity_throttle \u00b7 Capacity throttle";
    case "turn_error":
      return "turn_error \u00b7 Turn error";
    case "watchdog_killed":
      return "watchdog_killed \u00b7 Watchdog killed";
    case "mcp_stdout_undelivered":
      return "mcp_stdout_undelivered \u00b7 MCP stdout undelivered";
  }
}

export function handoffSemanticsLabel(
  closedClass: ClosedClass | null | undefined,
  exactUndelivered = false,
): string | null {
  if (exactUndelivered || closedClass === "mcp_stdout_undelivered") {
    return EXACT_PRESERVED_RESPONSE_LABEL;
  }
  if (closedClass === "quota_exhausted") {
    return QUOTA_HANDOFF_LABEL;
  }
  return null;
}

export function lookupIncident(
  snapshot: PeerHealthSnapshot | null,
  incidentId: string | null,
): PeerIncident | null {
  if (!snapshot || !incidentId) {
    return null;
  }
  return (
    snapshot.activeIncidents.find((incident) => incident.incidentId === incidentId) ??
    snapshot.recentIncidents.find((incident) => incident.incidentId === incidentId) ??
    null
  );
}

export function presentedHandoffLabel(
  selection: HandoffSelection,
  snapshot: PeerHealthSnapshot | null,
): string {
  const incident = lookupIncident(snapshot, selection.incidentId);
  const semantics = handoffSemanticsLabel(incident?.class, selection.exactUndelivered);
  const parts: string[] = [];
  const returned = selection.label.trim();
  if (returned) {
    parts.push(returned);
  }
  if (semantics && !parts.includes(semantics)) {
    parts.push(semantics);
  }
  return parts.join(" \u00b7 ");
}

export function applyOpenHandoff(
  selection: HandoffSelection,
  snapshot: PeerHealthSnapshot | null,
): AppliedHandoff {
  return {
    event: selection.event,
    replaceEvent: selection.event != null,
    label: presentedHandoffLabel(selection, snapshot),
  };
}

export function peerHealthActions(args: {
  snapshot: PeerHealthSnapshot | null;
  busy: boolean;
}): PeerHealthActionModel {
  const ready = args.snapshot != null && !args.busy;
  return {
    muteLabel: MUTE_CONTROL_LABEL,
    muteChecked: args.snapshot?.muted === true,
    muteEnabled: ready,
    nextMuted: args.snapshot ? !args.snapshot.muted : true,
    testChimeLabel: TEST_CHIME_LABEL,
    testChimeEnabled: ready,
    openHandoffLabel: OPEN_LATEST_HANDOFF_LABEL,
    openHandoffEnabled: ready,
  };
}

export function canAcknowledgeIncident(incident: PeerIncident, busy: boolean): boolean {
  return !busy && !incident.acknowledged;
}

export function formatCodexSample(sample: CodexSample | null): string {
  if (!sample) {
    return NO_CODEX_SAMPLE_LABEL;
  }
  const parts = ["Codex usage sample"];
  if (sample.usedPercent != null && Number.isFinite(sample.usedPercent)) {
    parts.push(`${sample.usedPercent}% used`);
  }
  if (sample.planType) {
    parts.push(`plan ${sample.planType}`);
  }
  if (sample.resetsAt) {
    parts.push(`resets ${sample.resetsAt}`);
  }
  if (sample.rateLimitReachedType) {
    parts.push(`rate limit ${sample.rateLimitReachedType}`);
  }
  parts.push(`as of ${formatTimestamp(sample.asOfMs)}`);
  return parts.join(" \u00b7 ");
}

export function formatGrokObservation(observation: GrokObservation | null): string {
  if (!observation) {
    return NO_GROK_OBSERVATION_LABEL;
  }
  const parts = [
    "Grok observation",
    closedClassLabel(observation.class),
    observation.success ? "successful" : "unsuccessful",
  ];
  if (observation.httpStatus != null && Number.isFinite(observation.httpStatus)) {
    parts.push(`HTTP ${Math.trunc(observation.httpStatus)}`);
  }
  if (observation.providerCode) {
    parts.push(`code ${observation.providerCode}`);
  }
  parts.push(`as of ${formatTimestamp(observation.asOfMs)}`);
  return parts.join(" \u00b7 ");
}

export function formatPeerHealthDiagnostics(diagnostics: PeerHealthDiagnostics): string {
  const parts: string[] = [];
  if (diagnostics.snapshotMissing) {
    parts.push("snapshot missing");
  }
  if (diagnostics.snapshotMalformed) {
    parts.push("snapshot malformed");
  }
  if (diagnostics.snapshotLocked) {
    parts.push("snapshot locked");
  }
  if (diagnostics.journalIncompleteTrailing) {
    parts.push("journal incomplete trailing");
  }
  parts.push(
    `malformed journal ${truncCount(diagnostics.malformedJournalLines)}`,
    `oversized journal ${truncCount(diagnostics.oversizedJournalLines)}`,
    `unsupported journal ${truncCount(diagnostics.unsupportedJournalRecords)}`,
    `quarantined inbox ${truncCount(diagnostics.quarantinedInbox)}`,
    `malformed inbox ${truncCount(diagnostics.malformedInbox)}`,
    `sound failures ${truncCount(diagnostics.soundFailures)}`,
    `footer missing ${truncCount(diagnostics.footerMissing)}`,
    `Codex sample failures ${truncCount(diagnostics.codexSampleFailures)}`,
  );
  if (diagnostics.lastCodexSampleFailureMs != null) {
    parts.push(
      `last Codex sample failure ${formatTimestamp(diagnostics.lastCodexSampleFailureMs)}`,
    );
  }
  return parts.join(" \u00b7 ");
}

export function presentPeerIncident(incident: PeerIncident, busy: boolean): PresentedPeerIncident {
  const recovered =
    incident.recoveredMs == null ? null : `recovered ${formatTimestamp(incident.recoveredMs)}`;
  const ids = [
    incident.sessionId ? `session ${incident.sessionId}` : null,
    incident.exchangeId ? `exchange ${incident.exchangeId}` : null,
    incident.eventId ? `event ${incident.eventId}` : null,
    `incident ${incident.incidentId}`,
  ].filter((part): part is string => part != null);
  const meta = [
    `opened ${formatTimestamp(incident.openedMs)}`,
    `as of ${formatTimestamp(incident.asOfMs)}`,
    recovered,
    ...ids,
  ]
    .filter((part): part is string => part != null)
    .join(" \u00b7 ");
  return {
    incidentId: incident.incidentId,
    title: `${incident.class} \u00b7 ${incident.source} \u00b7 ${incident.status}`,
    meta,
    class: incident.class,
    source: incident.source,
    status: incident.status,
    acknowledged: incident.acknowledged,
    canAcknowledge: canAcknowledgeIncident(incident, busy),
    handoffHint: handoffSemanticsLabel(incident.class),
  };
}

export function presentPeerHealth(state: PeerHealthUiState): PresentedPeerHealth {
  const actions = peerHealthActions({ snapshot: state.snapshot, busy: state.busy });
  if (!state.snapshot) {
    const availabilityKind: PeerHealthAvailabilityKind = state.loading
      ? "loading"
      : state.error
        ? "error"
        : "loading";
    return {
      title: PEER_HEALTH_TITLE,
      caption: PEER_HEALTH_CAPTION,
      availability: state.loading || !state.error ? LOADING_PEER_HEALTH_LABEL : NO_SNAPSHOT_YET_LABEL,
      availabilityKind,
      silenceNote: null,
      mutedLabel: CHIME_AUDIBLE_LABEL,
      unreadLabel: formatCount(0, "unacknowledged active incident"),
      generatedLabel: NO_SNAPSHOT_YET_LABEL,
      codexLabel: NO_SNAPSHOT_YET_LABEL,
      grokLabel: NO_SNAPSHOT_YET_LABEL,
      diagnosticsLabel: NO_SNAPSHOT_YET_LABEL,
      activeHeading: "Active incidents",
      recentHeading: "Recent incidents",
      activeEmpty: NO_ACTIVE_INCIDENTS_LABEL,
      recentEmpty: NO_RECENT_INCIDENTS_LABEL,
      activeIncidents: [],
      recentIncidents: [],
      actions,
      handoffLabel: noticeLabel(state.handoffLabel, state.chimeStatus),
      error: state.error,
    };
  }

  const snapshot = state.snapshot;
  const availabilityKind: PeerHealthAvailabilityKind = snapshot.unavailable
    ? "unavailable"
    : snapshot.stale
      ? "stale"
      : "current";
  const activeIncidents = snapshot.activeIncidents.map((incident) =>
    presentPeerIncident(incident, state.busy),
  );
  const recentIncidents = snapshot.recentIncidents.map((incident) =>
    presentPeerIncident(incident, state.busy),
  );
  return {
    title: PEER_HEALTH_TITLE,
    caption: PEER_HEALTH_CAPTION,
    availability: snapshotAvailabilityLabel(snapshot),
    availabilityKind,
    silenceNote: silenceNoteForSnapshot(snapshot),
    mutedLabel: snapshot.muted ? CHIME_MUTED_LABEL : CHIME_AUDIBLE_LABEL,
    unreadLabel: formatCount(snapshot.unreadCount, "unacknowledged active incident"),
    generatedLabel: [
      `Generated ${formatTimestamp(snapshot.generatedMs)}`,
      `As of ${snapshot.asOfMs == null ? "unknown" : formatTimestamp(snapshot.asOfMs)}`,
      `schema ${Math.trunc(snapshot.schemaVersion)}`,
    ].join(" \u00b7 "),
    codexLabel: formatCodexSample(snapshot.latestCodexSample),
    grokLabel: formatGrokObservation(snapshot.latestGrokObservation),
    diagnosticsLabel: formatPeerHealthDiagnostics(snapshot.diagnostics),
    activeHeading: "Active incidents",
    recentHeading: "Recent incidents",
    activeEmpty: activeIncidents.length === 0 ? NO_ACTIVE_INCIDENTS_LABEL : null,
    recentEmpty: recentIncidents.length === 0 ? NO_RECENT_INCIDENTS_LABEL : null,
    activeIncidents,
    recentIncidents,
    actions,
    handoffLabel: noticeLabel(state.handoffLabel, state.chimeStatus),
    error: state.error,
  };
}

export function buildPeerHealthSection(): PeerHealthNodes {
  const caption = el("p", { className: "peer-health-caption", text: PEER_HEALTH_CAPTION });
  const availability = el("p", {
    className: "peer-health-availability",
    attrs: { role: "status" },
  });
  const silenceNote = el("p", { className: "peer-health-silence" });
  const errorLine = el("p", {
    className: "peer-health-error",
    attrs: { role: "status" },
  });
  errorLine.hidden = true;
  const mutedLine = el("p", { className: "peer-health-meta" });
  const unreadLine = el("p", { className: "peer-health-meta" });
  const generatedLine = el("p", { className: "peer-health-meta" });
  const codexLine = el("p", { className: "peer-health-evidence-text" });
  const grokLine = el("p", { className: "peer-health-evidence-text" });
  const diagnosticsLine = el("p", { className: "peer-health-diagnostics" });
  const activeEmpty = el("p", { className: "panel-empty" });
  const activeList = el("ul", {
    className: "peer-health-incident-list",
    attrs: { "aria-label": "Active peer-health incidents" },
  });
  const recentEmpty = el("p", { className: "panel-empty" });
  const recentList = el("ul", {
    className: "peer-health-incident-list",
    attrs: { "aria-label": "Recent peer-health incidents" },
  });
  const handoffLine = el("p", {
    className: "peer-health-handoff",
    attrs: { role: "status" },
  });
  const muted = el("input", { attrs: { type: "checkbox" } });
  muted.id = "peer-health-muted";
  const testChime = button(TEST_CHIME_LABEL, "action", () => undefined, {
    "aria-label": TEST_CHIME_LABEL,
  });
  const openHandoff = button(OPEN_LATEST_HANDOFF_LABEL, "action", () => undefined, {
    "aria-label": OPEN_LATEST_HANDOFF_LABEL,
  });
  const section = el("section", {
    className: "panel panel-peer-health",
    attrs: { "aria-label": PEER_HEALTH_TITLE },
    children: [
      el("h2", { text: PEER_HEALTH_TITLE }),
      caption,
      availability,
      silenceNote,
      errorLine,
      el("div", {
        className: "peer-health-summary",
        children: [mutedLine, unreadLine, generatedLine],
      }),
      el("div", {
        className: "peer-health-evidence",
        children: [
          el("div", {
            className: "peer-health-card peer-health-card-codex",
            children: [
              el("p", { className: "peer-health-kicker", text: "Codex" }),
              codexLine,
            ],
          }),
          el("div", {
            className: "peer-health-card peer-health-card-grok",
            children: [
              el("p", { className: "peer-health-kicker", text: "Grok" }),
              grokLine,
            ],
          }),
        ],
      }),
      el("div", {
        className: "peer-health-incidents",
        children: [
          el("div", {
            className: "peer-health-incident-column",
            children: [
              el("p", { className: "peer-health-kicker", text: "Active incidents" }),
              activeEmpty,
              activeList,
            ],
          }),
          el("div", {
            className: "peer-health-incident-column",
            children: [
              el("p", { className: "peer-health-kicker", text: "Recent incidents" }),
              recentEmpty,
              recentList,
            ],
          }),
        ],
      }),
      diagnosticsLine,
      handoffLine,
      el("div", {
        className: "peer-health-controls",
        children: [
          labelledControl(MUTE_CONTROL_LABEL, muted, "field field-check"),
          testChime,
          openHandoff,
        ],
      }),
    ],
  });
  return {
    section,
    caption,
    availability,
    silenceNote,
    errorLine,
    mutedLine,
    unreadLine,
    generatedLine,
    codexLine,
    grokLine,
    diagnosticsLine,
    activeEmpty,
    activeList,
    recentEmpty,
    recentList,
    handoffLine,
    muted,
    testChime,
    openHandoff,
  };
}

export function paintPeerHealth(
  nodes: PeerHealthNodes,
  view: PresentedPeerHealth,
  onAcknowledge: (incidentId: string) => void,
): void {
  setText(nodes.caption, view.caption);
  setText(nodes.availability, view.availability);
  nodes.availability.className = availabilityClass(view.availabilityKind);
  if (view.silenceNote) {
    nodes.silenceNote.hidden = false;
    setText(nodes.silenceNote, view.silenceNote);
  } else {
    nodes.silenceNote.hidden = true;
    setText(nodes.silenceNote, "");
  }
  if (view.error) {
    nodes.errorLine.hidden = false;
    setText(nodes.errorLine, view.error);
  } else {
    nodes.errorLine.hidden = true;
    setText(nodes.errorLine, "");
  }
  setText(nodes.mutedLine, view.mutedLabel);
  setText(nodes.unreadLine, view.unreadLabel);
  setText(nodes.generatedLine, view.generatedLabel);
  setText(nodes.codexLine, view.codexLabel);
  setText(nodes.grokLine, view.grokLabel);
  setText(nodes.diagnosticsLine, view.diagnosticsLabel);
  renderIncidentList(nodes.activeList, nodes.activeEmpty, view.activeIncidents, view.activeEmpty, onAcknowledge);
  renderIncidentList(
    nodes.recentList,
    nodes.recentEmpty,
    view.recentIncidents,
    view.recentEmpty,
    onAcknowledge,
  );
  if (view.handoffLabel) {
    nodes.handoffLine.hidden = false;
    setText(nodes.handoffLine, view.handoffLabel);
  } else {
    nodes.handoffLine.hidden = true;
    setText(nodes.handoffLine, "");
  }
  if (document.activeElement !== nodes.muted) {
    nodes.muted.checked = view.actions.muteChecked;
  }
  nodes.muted.disabled = !view.actions.muteEnabled;
  nodes.testChime.disabled = !view.actions.testChimeEnabled;
  nodes.openHandoff.disabled = !view.actions.openHandoffEnabled;
}

function availabilityClass(kind: PeerHealthAvailabilityKind): string {
  if (kind === "unavailable" || kind === "stale") {
    return "peer-health-availability is-caution";
  }
  if (kind === "error") {
    return "peer-health-availability is-error";
  }
  return "peer-health-availability";
}

function renderIncidentList(
  list: HTMLUListElement,
  empty: HTMLParagraphElement,
  incidents: PresentedPeerIncident[],
  emptyLabel: string | null,
  onAcknowledge: (incidentId: string) => void,
): void {
  if (emptyLabel) {
    empty.hidden = false;
    setText(empty, emptyLabel);
  } else {
    empty.hidden = true;
    setText(empty, "");
  }
  list.replaceChildren();
  for (const incident of incidents) {
    const item = el("li", { className: "peer-health-incident" });
    item.append(
      el("p", { className: "record-title", text: incident.title }),
      el("p", { className: "record-meta", text: incident.meta }),
    );
    if (incident.handoffHint) {
      item.append(el("p", { className: "record-flag", text: incident.handoffHint }));
    }
    if (incident.canAcknowledge) {
      item.append(
        button(ACKNOWLEDGE_LABEL, "action peer-health-ack", () => onAcknowledge(incident.incidentId)),
      );
    } else if (incident.acknowledged) {
      item.append(el("p", { className: "record-meta", text: "Acknowledged" }));
    }
    list.append(item);
  }
}

function truncCount(value: number): number {
  return Number.isFinite(value) ? Math.trunc(value) : 0;
}

function emptyToNull(value: string | null | undefined): string | null {
  if (value == null) {
    return null;
  }
  const trimmed = value.trim();
  return trimmed.length === 0 ? null : trimmed;
}

function noticeLabel(handoffLabel: string | null, chimeStatus: string | null | undefined): string | null {
  const parts = [emptyToNull(handoffLabel), emptyToNull(chimeStatus)].filter(
    (part): part is string => part != null,
  );
  return parts.length === 0 ? null : parts.join(" \u00b7 ");
}
