import type { Diagnostics, SourceState, UnderlayState, ViewerStatus } from "../contracts";

export const PENDING_LABEL = "Request logged; no response event yet";
export const EXTRACTED_TASK_LABEL = "Extracted task";
export const PARLEY_ERROR_LABEL = "Parley execution error";

const pad = (value: number): string => String(value).padStart(2, "0");

export function formatTimestamp(ms: number | null, utc = false): string {
  if (ms == null || !Number.isFinite(ms)) {
    return "No timestamp";
  }
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) {
    return "No timestamp";
  }
  const year = utc ? date.getUTCFullYear() : date.getFullYear();
  const month = (utc ? date.getUTCMonth() : date.getMonth()) + 1;
  const day = utc ? date.getUTCDate() : date.getDate();
  const hour = utc ? date.getUTCHours() : date.getHours();
  const minute = utc ? date.getUTCMinutes() : date.getMinutes();
  const second = utc ? date.getUTCSeconds() : date.getSeconds();
  const stamp = `${year}-${pad(month)}-${pad(day)} ${pad(hour)}:${pad(minute)}:${pad(second)}`;
  return utc ? `${stamp} UTC` : stamp;
}

export function formatRoute(from: string, to: string): string {
  return `${from} \u2192 ${to}`;
}

export function formatEventType(eventType: "request" | "response" | "error"): string {
  switch (eventType) {
    case "request":
      return "Request";
    case "response":
      return "Response";
    case "error":
      return PARLEY_ERROR_LABEL;
  }
}

export function formatSourceState(state: SourceState): string {
  switch (state) {
    case "none":
      return "No event log selected";
    case "missing":
      return "Event log is missing";
    case "watching":
      return "Watching";
    case "degraded":
      return "Source degraded";
  }
}

export function formatUnderlayState(state: UnderlayState): string {
  switch (state) {
    case "detached":
      return "detached";
    case "attaching":
      return "attaching";
    case "attached":
      return "attached";
    case "degraded":
      return "degraded";
  }
}

export function formatCount(value: number, singular: string, plural = `${singular}s`): string {
  const n = Number.isFinite(value) ? Math.trunc(value) : 0;
  return `${n} ${n === 1 ? singular : plural}`;
}

export function formatCharacterCount(value: number): string {
  return `${Number.isFinite(value) ? Math.trunc(value) : 0} characters`;
}

export function formatDuration(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) {
    return "No duration";
  }
  return `${Math.trunc(ms)} ms`;
}

export function formatDiagnostics(diagnostics: Diagnostics): string {
  const parts = [
    `malformed ${Math.trunc(diagnostics.malformedLines)}`,
    `oversized ${Math.trunc(diagnostics.oversizedLines)}`,
    `unsupported ${Math.trunc(diagnostics.unsupportedRecords)}`,
    `duplicates ${Math.trunc(diagnostics.duplicateEvents)}`,
    `I/O ${Math.trunc(diagnostics.ioErrors)}`,
    `aliases ${Math.trunc(diagnostics.aliasCollisions)}`,
  ];
  if (diagnostics.lastError) {
    parts.push(`last error: ${diagnostics.lastError}`);
  }
  return parts.join("; ");
}

export function formatSourceLine(status: ViewerStatus): string {
  const base = formatSourceState(status.sourceState);
  const sources = `. ${formatCount(status.sources.length, "source")}`;
  const counts = `; ${formatCount(status.sessionCount, "session")}; ${formatCount(status.exchangeCount, "exchange")}`;
  return `${base}${sources}${counts}`;
}

export function formatRuntimeHealth(status: ViewerStatus): string {
  const tray = status.trayAvailable ? "Tray available" : "Tray unavailable";
  const underlay = `Underlay ${formatUnderlayState(status.underlayState)}`;
  const widget = status.widgetVisible ? "Widget visible" : "Widget hidden";
  return `${tray}. ${underlay}; ${widget}`;
}

export function idleWidgetLabel(state: SourceState | null): string {
  switch (state) {
    case "missing":
      return "Event log is missing";
    case "degraded":
      return "Event log source is degraded";
    case "watching":
      return "Waiting for conversation events";
    case "none":
    case null:
      return "No event log selected";
  }
}
