import type {
  PeerActivityDiagnostics,
  PeerActivityEvent,
  PeerActivitySnapshot,
  PeerHandoffItem,
} from "../contracts";

import { el, setText } from "./dom";
import { formatCount, formatTimestamp } from "./format";

export const PEER_ACTIVITY_TITLE = "Pending handoffs and peer activity";
export const PEER_ACTIVITY_CAPTION =
  "Read-only handoff evidence. Activity is not a failure verdict, silence grants no authority, and receipt confirms delivery only.";
export const NO_HANDOFFS_LABEL = "No handoff evidence is available.";
export const LOADING_ACTIVITY_LABEL = "Loading handoff evidence\u2026";

export interface PresentedActivityEvent {
  label: string;
  meta: string;
}

export interface PresentedHandoff {
  jobId: string;
  heading: string;
  identity: string;
  lifecycle: string;
  timestamps: string;
  freshness: string;
  receipt: string;
  alert: string | null;
  diagnostic: string | null;
  excerpt: string | null;
  excerptLabel: string | null;
  activities: PresentedActivityEvent[];
  reportStatus: string;
  report: string | null;
}

export interface PresentedPeerActivity {
  title: string;
  caption: string;
  availability: string;
  summary: string;
  diagnostics: string;
  error: string | null;
  empty: string | null;
  handoffs: PresentedHandoff[];
}

export interface PeerActivityUiState {
  snapshot: PeerActivitySnapshot | null;
  error: string | null;
  loading: boolean;
}

export interface PeerActivityNodes {
  section: HTMLElement;
  availability: HTMLParagraphElement;
  summary: HTMLParagraphElement;
  diagnostics: HTMLParagraphElement;
  error: HTMLParagraphElement;
  empty: HTMLParagraphElement;
  list: HTMLUListElement;
}

export function presentPeerActivity(state: PeerActivityUiState): PresentedPeerActivity {
  if (!state.snapshot) {
    return {
      title: PEER_ACTIVITY_TITLE,
      caption: PEER_ACTIVITY_CAPTION,
      availability: state.loading ? LOADING_ACTIVITY_LABEL : NO_HANDOFFS_LABEL,
      summary: "Assessment unavailable; no peer state is inferred.",
      diagnostics: "No handoff diagnostics yet.",
      error: state.error,
      empty: NO_HANDOFFS_LABEL,
      handoffs: [],
    };
  }
  const snapshot = state.snapshot;
  const source = snapshot.source.kind.replaceAll("_", " ");
  const availability = snapshot.unavailable
    ? `Handoff evidence unavailable (${snapshot.unavailable}).`
    : `Handoff evidence loaded from ${source}.`;
  const truncation = snapshot.truncated ? "; newest bounded page" : "";
  return {
    title: PEER_ACTIVITY_TITLE,
    caption: PEER_ACTIVITY_CAPTION,
    availability,
    summary: `${formatCount(snapshot.shownCount, "shown handoff")} of ${formatCount(snapshot.totalCount, "recorded handoff")}; assessment ${snapshot.assessment}; generated ${formatTimestamp(snapshot.generatedMs)}${truncation}`,
    diagnostics: formatActivityDiagnostics(snapshot.diagnostics),
    error: state.error,
    empty: snapshot.handoffs.length === 0 ? NO_HANDOFFS_LABEL : null,
    handoffs: snapshot.handoffs.map((handoff) => presentHandoff(handoff, snapshot.generatedMs)),
  };
}

export function peerActivityRevision(snapshot: PeerActivitySnapshot): string {
  return JSON.stringify({
    source: snapshot.source,
    unavailable: snapshot.unavailable,
    diagnostics: snapshot.diagnostics,
    shownCount: snapshot.shownCount,
    totalCount: snapshot.totalCount,
    truncated: snapshot.truncated,
    handoffs: snapshot.handoffs.map((handoff) => ({
      ...handoff,
      reportText: handoff.reportText?.length ?? null,
    })),
  });
}

export function presentHandoff(handoff: PeerHandoffItem, generatedMs: number): PresentedHandoff {
  const state = handoff.state ?? "unavailable";
  const phase = handoff.phase ?? "unavailable";
  const process = handoff.processState ?? "unavailable";
  const reportStatus = `Report ${handoff.reportAvailability}`;
  return {
    jobId: handoff.jobId,
    heading: `Job ${handoff.jobId}`,
    identity: [
      handoff.handoffId ? `handoff ${handoff.handoffId}` : null,
      handoff.sourceSessionId ? `source session ${handoff.sourceSessionId}` : null,
      handoff.targetSessionId ? `target session ${handoff.targetSessionId}` : null,
    ]
      .filter((value): value is string => value != null)
      .join("; "),
    lifecycle: `state ${state}; phase ${phase}; process ${process}`,
    timestamps: [
      handoff.createdAtMs == null ? null : `created ${formatTimestamp(handoff.createdAtMs)}`,
      handoff.updatedAtMs == null ? null : `updated ${formatTimestamp(handoff.updatedAtMs)}`,
      handoff.readyAtMs == null ? null : `ready ${formatTimestamp(handoff.readyAtMs)}`,
      handoff.deadlineMs == null ? null : `receipt deadline ${formatTimestamp(handoff.deadlineMs)}`,
    ]
      .filter((value): value is string => value != null)
      .join("; "),
    freshness: freshnessLabel(generatedMs, handoff.lastActivityMs),
    receipt:
      handoff.receiptAtMs == null
        ? "No Codex receipt recorded."
        : `Codex receipt recorded ${formatTimestamp(handoff.receiptAtMs)}; delivery only, not acceptance.`,
    alert: handoff.alertIncidentId ? `Explicit handoff alert ${handoff.alertIncidentId}` : null,
    diagnostic: handoff.recordDiagnostic
      ? `Record evidence unavailable (${handoff.recordDiagnostic}).`
      : null,
    excerpt: handoff.excerptText,
    excerptLabel:
      handoff.excerptText == null
        ? null
        : handoff.excerptTruncated
          ? "Exact visible-output excerpt (bounded and truncated)"
          : "Exact visible-output excerpt (bounded)",
    activities: handoff.activities.map(presentActivityEvent),
    reportStatus,
    report: handoff.reportAvailability === "available" ? handoff.reportText : null,
  };
}

export function freshnessLabel(generatedMs: number, lastActivityMs: number | null): string {
  if (lastActivityMs == null) {
    return "No activity timestamp recorded; no peer state is inferred.";
  }
  const age = Math.max(0, Math.trunc(generatedMs - lastActivityMs));
  return `Last sanitized activity ${formatTimestamp(lastActivityMs)}; ${age} ms before this snapshot.`;
}

export function formatActivityDiagnostics(diagnostics: PeerActivityDiagnostics): string {
  const flags = [
    diagnostics.rootMissing ? "root missing" : null,
    diagnostics.rootLocked ? "root locked" : null,
    diagnostics.rootMalformed ? "root malformed" : null,
    diagnostics.jobsMissing ? "jobs missing" : null,
    diagnostics.jobsLocked ? "jobs locked" : null,
    diagnostics.jobsMalformed ? "jobs malformed" : null,
  ].filter((value): value is string => value != null);
  return [
    ...flags,
    `skipped ${count(diagnostics.skippedEntries)}`,
    `missing jobs ${count(diagnostics.missingJobs)}`,
    `malformed jobs ${count(diagnostics.malformedJobs)}`,
    `locked jobs ${count(diagnostics.lockedJobs)}`,
    `unavailable jobs ${count(diagnostics.unavailableJobs)}`,
    `report mismatches ${count(diagnostics.reportMismatches)}`,
    `missing reports ${count(diagnostics.reportMissing)}`,
    `locked reports ${count(diagnostics.reportLocked)}`,
    `malformed reports ${count(diagnostics.reportMalformed)}`,
  ].join("; ");
}

function presentActivityEvent(activity: PeerActivityEvent): PresentedActivityEvent {
  const details = [activity.toolName ? `tool ${activity.toolName}` : null, activity.status]
    .filter((value): value is string => value != null)
    .join("; ");
  return {
    label: activity.class,
    meta: `${formatTimestamp(activity.timestampMs)}${details ? `; ${details}` : ""}`,
  };
}

function count(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

export function buildPeerActivitySection(): PeerActivityNodes {
  const availability = el("p", { className: "peer-activity-availability" });
  const summary = el("p", { className: "peer-activity-meta" });
  const diagnostics = el("p", { className: "peer-activity-diagnostics" });
  const error = el("p", { className: "peer-activity-error", attrs: { role: "status" } });
  error.hidden = true;
  const empty = el("p", { className: "panel-empty" });
  const list = el("ul", {
    className: "peer-activity-list",
    attrs: { "aria-label": "Pending handoffs and peer activity" },
  });
  const section = el("section", {
    className: "panel panel-peer-activity",
    attrs: { "aria-label": PEER_ACTIVITY_TITLE },
    children: [
      el("h2", { text: PEER_ACTIVITY_TITLE }),
      el("p", { className: "peer-activity-caption", text: PEER_ACTIVITY_CAPTION }),
      availability,
      summary,
      error,
      empty,
      list,
      diagnostics,
    ],
  });
  return { section, availability, summary, diagnostics, error, empty, list };
}

export function paintPeerActivity(nodes: PeerActivityNodes, view: PresentedPeerActivity): void {
  setText(nodes.availability, view.availability);
  setText(nodes.summary, view.summary);
  setText(nodes.diagnostics, view.diagnostics);
  nodes.error.hidden = view.error == null;
  setText(nodes.error, view.error ?? "");
  nodes.empty.hidden = view.empty == null;
  setText(nodes.empty, view.empty ?? "");
  nodes.list.replaceChildren(...view.handoffs.map(renderHandoff));
}

function renderHandoff(handoff: PresentedHandoff): HTMLLIElement {
  const activities = el("ul", { className: "peer-activity-events" });
  for (const activity of handoff.activities) {
    activities.append(
      el("li", {
        children: [
          el("span", { className: "peer-activity-event-class", text: activity.label }),
          el("span", { className: "peer-activity-event-meta", text: activity.meta }),
        ],
      }),
    );
  }
  const report = el("details", {
    className: "peer-activity-report",
    children: [el("summary", { text: handoff.reportStatus })],
  });
  if (handoff.report != null) {
    report.append(
      el("pre", {
        className: "peer-activity-report-body",
        text: handoff.report,
        attrs: { tabindex: "0", "aria-label": `Exact durable report for ${handoff.jobId}` },
      }),
    );
  }
  return el("li", {
    className: "peer-activity-item",
    children: [
      el("h3", { text: handoff.heading }),
      meta(handoff.lifecycle),
      meta(handoff.identity),
      meta(handoff.timestamps),
      meta(handoff.freshness),
      meta(handoff.receipt),
      handoff.alert ? meta(handoff.alert) : null,
      handoff.diagnostic
        ? el("p", { className: "peer-activity-warning", text: handoff.diagnostic })
        : null,
      handoff.excerptLabel ? el("p", { className: "peer-activity-kicker", text: handoff.excerptLabel }) : null,
      handoff.excerpt == null
        ? null
        : el("pre", { className: "peer-activity-excerpt", text: handoff.excerpt }),
      handoff.activities.length === 0
        ? meta("No sanitized activity events recorded.")
        : el("div", {
            children: [
              el("p", { className: "peer-activity-kicker", text: "Sanitized activity events" }),
              activities,
            ],
          }),
      report,
    ],
  });
}

function meta(text: string): HTMLParagraphElement {
  return el("p", { className: "peer-activity-meta", text });
}
