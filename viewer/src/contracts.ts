export type SourceState = "none" | "missing" | "watching" | "degraded";
export type UnderlayState = "detached" | "attaching" | "attached" | "degraded";
export type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";
export type DesktopMode = "interactive" | "passive";
export type DesktopRuntimeState =
  | "passive"
  | "interactive-starting"
  | "interactive"
  | "passive-fallback";
export type DesktopFallbackReason =
  | "development-gate-closed"
  | "preference-passive"
  | "underlay-unavailable"
  | "surface-create-failed"
  | "surface-document-not-ready"
  | "surface-bounds-invalid"
  | "surface-style-invalid"
  | "surface-owner-invalid"
  | "surface-geometry-mismatch"
  | "surface-z-order-invalid"
  | "surface-restack-limit"
  | "explorer-lost"
  | "surface-destroy-failed";

export interface Diagnostics {
  malformedLines: number;
  oversizedLines: number;
  unsupportedRecords: number;
  duplicateEvents: number;
  ioErrors: number;
  aliasCollisions: number;
  lastError: string | null;
}

export interface SourceStatus {
  path: string;
  identity: string;
  sourceState: SourceState;
  generation: number;
  bytesRead: number;
  sessionCount: number;
  exchangeCount: number;
  lastEventTimestampMs: number | null;
  diagnostics: Diagnostics;
  aliasOf: string | null;
}

export interface ViewerStatus {
  sourceState: SourceState;
  generation: number;
  bytesRead: number;
  sessionCount: number;
  exchangeCount: number;
  lastEventTimestampMs: number | null;
  trayAvailable: boolean;
  underlayState: UnderlayState;
  desktopRuntimeState: DesktopRuntimeState;
  desktopFallbackReason: DesktopFallbackReason | null;
  widgetVisible: boolean;
  diagnostics: Diagnostics;
  sources: SourceStatus[];
}

export interface SessionSummary {
  sessionKey: string;
  sessionId: string;
  sourcePath: string;
  exchangeCount: number;
  latestTimestampMs: number;
  latestSource: string;
  latestTarget: string;
  latestExcerpt: string;
  excerptExtracted: boolean;
}

export interface SessionPage {
  items: SessionSummary[];
  nextCursor: number | null;
  total: number;
}

export interface MessagePreview {
  eventKey: string;
  eventId: string;
  eventType: "request" | "response" | "error";
  speaker: string;
  recipient: string;
  timestampMs: number;
  status: string;
  excerpt: string;
  excerptExtracted: boolean;
  contentLength: number;
}

export interface ExchangeSummary {
  exchangeKey: string;
  sessionKey: string;
  exchangeId: string;
  sessionId: string;
  sourcePath: string;
  timestampMs: number;
  request: MessagePreview | null;
  completion: MessagePreview | null;
  pendingLabel: string | null;
}

export interface ExchangePage {
  items: ExchangeSummary[];
  nextCursor: number | null;
  total: number;
}

export interface ConversationPage {
  historyToken: string;
  items: ExchangeSummary[];
  nextBeforeExchangeKey: string | null;
  hasEarlier: boolean;
  hasNewer: boolean;
  totalExchanges: number;
  anchorExchangeKey: string | null;
  resetRequired: boolean;
}

export interface SearchHit {
  eventKey: string;
  exchangeKey: string;
  sessionKey: string;
  eventId: string;
  exchangeId: string;
  sessionId: string;
  sourcePath: string;
  eventType: "request" | "response" | "error";
  timestampMs: number;
  excerpt: string;
  matchOffset: number;
}

export interface SearchPage {
  items: SearchHit[];
  nextCursor: number | null;
  total: number;
}

export interface EventContent {
  eventKey: string;
  exchangeKey: string;
  sessionKey: string;
  eventId: string;
  exchangeId: string;
  sessionId: string;
  sourcePath: string;
  eventType: "request" | "response" | "error";
  speaker: string;
  recipient: string;
  timestampMs: number;
  status: string;
  durationMs: number | null;
  error: string | null;
  content: string;
  context: ContextDiagnostics | null;
}

export interface ContextDiagnostics {
  source: string | null;
  mode: string | null;
  fromOffset: number | null;
  toOffset: number | null;
  recordCount: number | null;
  characterCount: number | null;
  truncated: boolean | null;
  recovery: string | null;
}

export type WidgetFeedProjection = "exact" | "current-request" | "speech" | "withheld";

export interface WidgetFeedMessage {
  eventKey: string;
  eventType: "request" | "response" | "error";
  speaker: string;
  recipient: string;
  timestampMs: number;
  status: string;
  body: string;
  fullCharacterLength: number;
  truncated: boolean;
  projection: WidgetFeedProjection;
  contextOmitted: boolean;
}

export interface WidgetFeedExchange {
  exchangeKey: string;
  sessionKey: string;
  timestampMs: number;
  request: WidgetFeedMessage | null;
  completion: WidgetFeedMessage | null;
  pendingLabel: string | null;
}

export interface WidgetFeedPage {
  historyToken: string;
  items: WidgetFeedExchange[];
  nextBeforeExchangeKey: string | null;
  hasEarlier: boolean;
  totalExchanges: number;
  totalEvents: number;
  resetRequired: boolean;
}

export interface WidgetSurfaceBoundsReport {
  left: number;
  top: number;
  width: number;
  height: number;
  viewportWidth: number;
  viewportHeight: number;
  devicePixelRatio: number;
}

export interface WidgetSurfaceActivityReport {
  phase: "poll" | "dom-paint" | "animation-frame";
  sequence: number;
  generation: number;
  changed: boolean;
  documentVisibility: "visible" | "hidden" | "prerender";
  monotonicMs: number;
}

export interface MonitorInfo {
  id: string;
  name: string;
  primary: boolean;
}

export interface ViewerSettings {
  selectedLog: string | null;
  selectedLogs: string[];
  monitorId: string | null;
  corner: Corner;
  offsetX: number;
  offsetY: number;
  width: number;
  height: number;
  launchAtLogin: boolean;
  desktopMode: DesktopMode;
}

export const DEFAULT_SETTINGS: ViewerSettings = {
  selectedLog: null,
  selectedLogs: [],
  monitorId: null,
  corner: "top-right",
  offsetX: 24,
  offsetY: 24,
  width: 720,
  height: 560,
  launchAtLogin: false,
  desktopMode: "interactive",
};

export const CLOSED_CLASSES = [
  "usage_sample",
  "quota_exhausted",
  "capacity_throttle",
  "turn_error",
  "watchdog_killed",
  "mcp_stdout_undelivered",
  "handoff_unacknowledged",
] as const;
export type ClosedClass = (typeof CLOSED_CLASSES)[number];

export const INCIDENT_CLASSES = ["quota_exhausted", "mcp_stdout_undelivered"] as const;
export type IncidentClass = (typeof INCIDENT_CLASSES)[number];

export function isIncidentClass(value: ClosedClass): value is IncidentClass {
  return INCIDENT_CLASSES.some((incidentClass) => incidentClass === value);
}

export const PEER_HEALTH_SOURCES = ["codex", "grok", "parley", "viewer"] as const;
export type PeerHealthSource = (typeof PEER_HEALTH_SOURCES)[number];

export const PEER_INCIDENT_STATUSES = ["active", "recovered"] as const;
export type PeerIncidentStatus = (typeof PEER_INCIDENT_STATUSES)[number];

export const PEER_HEALTH_UNAVAILABLE_REASONS = [
  "missing",
  "malformed",
  "locked",
  "arguments_not_allowed",
] as const;
export type PeerHealthUnavailableReason = (typeof PEER_HEALTH_UNAVAILABLE_REASONS)[number];

export interface CodexSample {
  usedPercent: number | null;
  resetsAt: string | null;
  planType: string | null;
  rateLimitReachedType: string | null;
  asOfMs: number;
}

export interface GrokObservation {
  class: ClosedClass;
  asOfMs: number;
  success: boolean;
  httpStatus: number | null;
  providerCode: string | null;
}

export interface PeerIncident {
  incidentId: string;
  class: ClosedClass;
  source: PeerHealthSource;
  status: PeerIncidentStatus;
  openedMs: number;
  asOfMs: number;
  recoveredMs: number | null;
  acknowledged: boolean;
  sessionId: string | null;
  eventId: string | null;
  exchangeId: string | null;
}

export interface PeerHealthUnavailable {
  reason: PeerHealthUnavailableReason;
}

export interface PeerHealthDiagnostics {
  snapshotMissing: boolean;
  snapshotMalformed: boolean;
  snapshotLocked: boolean;
  journalIncompleteTrailing: boolean;
  malformedJournalLines: number;
  oversizedJournalLines: number;
  unsupportedJournalRecords: number;
  quarantinedInbox: number;
  malformedInbox: number;
  soundFailures: number;
  footerMissing: number;
  codexSampleFailures: number;
  lastCodexSampleFailureMs: number | null;
}

export interface PeerHealthSnapshot {
  schemaVersion: number;
  generatedMs: number;
  asOfMs: number | null;
  muted: boolean;
  unreadCount: number;
  latestCodexSample: CodexSample | null;
  latestGrokObservation: GrokObservation | null;
  activeIncidents: PeerIncident[];
  recentIncidents: PeerIncident[];
  unavailable: PeerHealthUnavailable | null;
  stale: boolean;
  diagnostics: PeerHealthDiagnostics;
}

export interface HandoffSelection {
  event: EventContent | null;
  label: string;
  incidentId: string | null;
  exactUndelivered: boolean;
  diagnostic: string | null;
}

export type PeerActivityUnavailableReason =
  | "missing"
  | "malformed"
  | "locked"
  | "unavailable";

export type HandoffReportAvailability =
  | "absent"
  | "available"
  | "missing"
  | "malformed"
  | "locked"
  | "unavailable";

export interface PeerActivityDiagnostics {
  rootMissing: boolean;
  rootLocked: boolean;
  rootMalformed: boolean;
  jobsMissing: boolean;
  jobsLocked: boolean;
  jobsMalformed: boolean;
  skippedEntries: number;
  missingJobs: number;
  malformedJobs: number;
  lockedJobs: number;
  unavailableJobs: number;
  reportMismatches: number;
  reportMissing: number;
  reportLocked: number;
  reportMalformed: number;
}

export interface PeerActivityEvent {
  class: string;
  timestampMs: number;
  toolName: string | null;
  status: string | null;
}

export interface PeerHandoffItem {
  jobId: string;
  handoffId: string | null;
  sourceSessionId: string | null;
  targetSessionId: string | null;
  state: string | null;
  phase: string | null;
  processState: string | null;
  createdAtMs: number | null;
  updatedAtMs: number | null;
  lastActivityMs: number | null;
  readyAtMs: number | null;
  deadlineMs: number | null;
  receiptAtMs: number | null;
  alertIncidentId: string | null;
  recordDiagnostic: PeerActivityUnavailableReason | null;
  excerptText: string | null;
  excerptTruncated: boolean;
  activities: PeerActivityEvent[];
  reportAvailability: HandoffReportAvailability;
  reportText: string | null;
}

export interface PeerActivitySnapshot {
  generatedMs: number;
  assessment: "not_inferred";
  source: {
    kind: "environment" | "local_app_data" | "unconfigured";
  };
  unavailable: PeerActivityUnavailableReason | null;
  diagnostics: PeerActivityDiagnostics;
  shownCount: number;
  totalCount: number;
  truncated: boolean;
  handoffs: PeerHandoffItem[];
}

export const DEFAULT_PEER_HEALTH_DIAGNOSTICS: PeerHealthDiagnostics = {
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
};
