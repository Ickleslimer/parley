import type { MessagePreview, ViewerStatus } from "../contracts";

import { PARLEY_ERROR_LABEL, PENDING_LABEL } from "./format";

export { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./format";

export function pendingStatusLabel(label: string | null): string | null {
  return label;
}

export function exactPendingLabel(): string {
  return PENDING_LABEL;
}

export function parleyErrorLabel(): string {
  return PARLEY_ERROR_LABEL;
}

export function isParleyError(
  preview: Pick<MessagePreview, "eventType"> | null | undefined,
): boolean {
  return preview?.eventType === "error";
}

export function completionStatusLabel(args: {
  completion: Pick<MessagePreview, "eventType"> | null;
  pendingLabel: string | null;
}): string | null {
  if (args.completion && isParleyError(args.completion)) {
    return PARLEY_ERROR_LABEL;
  }
  if (args.completion) {
    return "Completion";
  }
  if (args.pendingLabel) {
    return args.pendingLabel;
  }
  return null;
}

export function degradedBanner(status: ViewerStatus | null): string | null {
  if (!status) {
    return null;
  }
  const trayDegraded = status.trayAvailable === false;
  const underlayDegraded = status.underlayState === "degraded";
  if (trayDegraded && underlayDegraded) {
    return "System tray and desktop underlay are degraded. The widget may be hidden. Exit remains available.";
  }
  if (trayDegraded) {
    return "System tray is unavailable. Exit remains available.";
  }
  if (underlayDegraded) {
    return "Desktop underlay is degraded. The widget may be hidden.";
  }
  return null;
}

export function loadErrorLabel(action: string): string {
  return `Unable to ${action}`;
}
