export type SourceState = "none" | "missing" | "watching" | "degraded";
export type UnderlayState = "detached" | "attaching" | "attached" | "degraded";
export type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

export interface Diagnostics {
  malformedLines: number;
  oversizedLines: number;
  unsupportedRecords: number;
  duplicateEvents: number;
  ioErrors: number;
  lastError: string | null;
}

export interface ViewerStatus {
  sourcePath: string | null;
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
}

export interface SessionSummary {
  sessionId: string;
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
  exchangeId: string;
  sessionId: string;
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
  eventId: string;
  exchangeId: string;
  sessionId: string;
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
  eventId: string;
  exchangeId: string;
  sessionId: string;
  eventType: "request" | "response" | "error";
  speaker: string;
  recipient: string;
  timestampMs: number;
  status: string;
  durationMs: number | null;
  error: string | null;
  content: string;
}

export interface WidgetSnapshot {
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
  monitorId: null,
  corner: "bottom-right",
  offsetX: 24,
  offsetY: 24,
  width: 560,
  height: 360,
  launchAtLogin: false,
};
