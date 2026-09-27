import type { PresentedExchange, PresentedMessage } from "./excerpt";

export type KnownSpeaker = "codex" | "grok";
export type BubbleKind = "speech" | "parley-error" | "pending" | "status";
export type TailDirection = "toward-codex" | "toward-grok" | "none";

export interface BubbleModel {
  kind: BubbleKind;
  speaker: KnownSpeaker | "unknown";
  displayName: string;
  tail: TailDirection;
  eventKey: string | null;
  excerpt: string;
  heading: string;
  timestamp: string;
  extractedLabel: string | null;
}

export interface WidgetSceneModel {
  banner: string | null;
  liveRevision: string;
  bubbles: BubbleModel[];
  idleLabel: string | null;
  loadError: string | null;
  sourceLabel: string;
}

export function normalizeSpeaker(value: string): KnownSpeaker | null {
  const normalized = value.trim().toLocaleLowerCase("en-US");
  return normalized === "codex" || normalized === "grok" ? normalized : null;
}

export function displaySpeaker(value: string): string {
  const known = normalizeSpeaker(value);
  if (known === "codex") {
    return "Codex";
  }
  if (known === "grok") {
    return "Grok";
  }
  return value.trim() || "Unknown agent";
}

export function tailForSpeaker(value: string): TailDirection {
  const known = normalizeSpeaker(value);
  return known ? `toward-${known}` : "none";
}

export function messageBubble(message: PresentedMessage): BubbleModel {
  if (message.eventType === "error") {
    return {
      kind: "parley-error",
      speaker: "unknown",
      displayName: "Parley",
      tail: "none",
      eventKey: message.eventKey,
      excerpt: message.excerpt,
      heading: message.heading,
      timestamp: message.timestamp,
      extractedLabel: message.extractedLabel,
    };
  }
  const known = normalizeSpeaker(message.speaker);
  return {
    kind: "speech",
    speaker: known ?? "unknown",
    displayName: displaySpeaker(message.speaker),
    tail: tailForSpeaker(message.speaker),
    eventKey: message.eventKey,
    excerpt: message.excerpt,
    heading: message.heading,
    timestamp: message.timestamp,
    extractedLabel: message.extractedLabel,
  };
}

export function pendingBubble(label: string, recipient: string): BubbleModel {
  const known = normalizeSpeaker(recipient);
  return {
    kind: "pending",
    speaker: known ?? "unknown",
    displayName: known ? displaySpeaker(recipient) : "Parley",
    tail: tailForSpeaker(recipient),
    eventKey: null,
    excerpt: label,
    heading: "Pending response",
    timestamp: "",
    extractedLabel: null,
  };
}

export function attributeExchange(exchange: PresentedExchange): BubbleModel[] {
  const bubbles: BubbleModel[] = [];
  if (exchange.request) {
    bubbles.push(messageBubble(exchange.request));
  }
  if (exchange.completion) {
    bubbles.push(messageBubble(exchange.completion));
  } else if (exchange.pendingLabel && exchange.request) {
    bubbles.push(pendingBubble(exchange.pendingLabel, exchange.request.recipient));
  }
  return bubbles;
}

export function liveRevision(bubbles: BubbleModel[]): string {
  return JSON.stringify(
    bubbles.map((bubble) => [
      bubble.kind,
      bubble.speaker,
      bubble.eventKey,
      bubble.heading,
      bubble.excerpt,
      bubble.timestamp,
      bubble.extractedLabel,
    ]),
  );
}
