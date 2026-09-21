export type SourceState = "none" | "missing" | "watching" | "degraded";
export type UnderlayState = "detached" | "attaching" | "attached" | "degraded";
export type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

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

export interface WidgetSnapshot {
  sessionKey: string | null;
  exchangeKey: string | null;
  sessionId: string | null;
  exchangeId: string | null;
  request: MessagePreview | null;
  completion: MessagePreview | null;
  pendingLabel: string | null;
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
}

export const DEFAULT_SETTINGS: ViewerSettings = {
  selectedLog: null,
  selectedLogs: [],
  monitorId: null,
  corner: "bottom-right",
  offsetX: 24,
  offsetY: 24,
  width: 560,
  height: 360,
  launchAtLogin: false,
};

export const CLOSED_CLASSES = [
  "usage_sample",
  "quota_exhausted",
  "capacity_throttle",
  "turn_error",
  "watchdog_killed",
  "mcp_stdout_undelivered",
] as const;
export type ClosedClass = (typeof CLOSED_CLASSES)[number];

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
