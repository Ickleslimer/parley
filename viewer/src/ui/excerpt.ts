import type { EventContent, ExchangeSummary, MessagePreview } from "../contracts";

import {
  EXTRACTED_TASK_LABEL,
  formatEventType,
  formatRoute,
  formatTimestamp,
  PARLEY_ERROR_LABEL,
} from "./format";
import { completionStatusLabel, isParleyError } from "./labels";

export type MessageRole = "request" | "completion";

export interface PresentedMessage {
  eventKey: string;
  eventId: string;
  eventType: MessagePreview["eventType"];
  heading: string;
  speaker: string;
  recipient: string;
  initial: string;
  route: string;
  timestamp: string;
  status: string;
  excerpt: string;
  extracted: boolean;
  extractedLabel: string | null;
}

export function participantInitial(name: string): string {
  const first = Array.from(name.trim())[0];
  return first === undefined ? "?" : first.toUpperCase();
}

export interface PresentedExchange {
  request: PresentedMessage | null;
  completion: PresentedMessage | null;
  pendingLabel: string | null;
  completionHeading: string | null;
}

export function presentMessage(
  preview: MessagePreview,
  role: MessageRole,
  utc = false,
): PresentedMessage {
  const heading =
    role === "completion" && isParleyError(preview)
      ? PARLEY_ERROR_LABEL
      : role === "request"
        ? "Request"
        : "Completion";
  return {
    eventKey: preview.eventKey,
    eventId: preview.eventId,
    eventType: preview.eventType,
    heading,
    speaker: preview.speaker,
    recipient: preview.recipient,
    initial: participantInitial(preview.speaker),
    route: formatRoute(preview.speaker, preview.recipient),
    timestamp: formatTimestamp(preview.timestampMs, utc),
    status: preview.status,
    excerpt: preview.excerpt,
    extracted: preview.excerptExtracted,
    extractedLabel: preview.excerptExtracted ? EXTRACTED_TASK_LABEL : null,
  };
}

export function presentExchange(
  exchange: Pick<ExchangeSummary, "request" | "completion" | "pendingLabel">,
  utc = false,
): PresentedExchange {
  const request = exchange.request ? presentMessage(exchange.request, "request", utc) : null;
  const completion = exchange.completion
    ? presentMessage(exchange.completion, "completion", utc)
    : null;
  return {
    request,
    completion,
    pendingLabel: completion ? null : exchange.pendingLabel,
    completionHeading: completionStatusLabel({
      completion: exchange.completion,
      pendingLabel: completion ? null : exchange.pendingLabel,
    }),
  };
}

export function searchHitHeading(eventType: MessagePreview["eventType"]): string {
  return formatEventType(eventType);
}

export function exactEventBody(
  event: Pick<EventContent, "eventType" | "content" | "error">,
): string {
  if (event.eventType === "error" && event.content.length === 0) {
    return event.error ?? "";
  }
  return event.content;
}
