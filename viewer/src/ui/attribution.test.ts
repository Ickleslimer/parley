import { describe, expect, it } from "vitest";

import type { PresentedMessage } from "./excerpt";
import {
  attributeExchange,
  displaySpeaker,
  messageBubble,
  normalizeSpeaker,
  pendingBubble,
} from "./attribution";

function message(overrides: Partial<PresentedMessage> = {}): PresentedMessage {
  return {
    eventKey: "event:1",
    eventId: "event-1",
    eventType: "request",
    heading: "Request",
    speaker: "codex",
    recipient: "grok",
    initial: "C",
    route: "codex to grok",
    timestamp: "2026-09-27 00:00:00",
    status: "ok",
    excerpt: "<img src=x onerror=alert(1)>",
    extracted: false,
    extractedLabel: null,
    ...overrides,
  };
}

describe("speaker attribution", () => {
  it("normalizes only exact Codex and Grok identifiers", () => {
    expect(normalizeSpeaker(" CODEX ")).toBe("codex");
    expect(normalizeSpeaker("Grok")).toBe("grok");
    expect(normalizeSpeaker("codex-helper")).toBeNull();
    expect(normalizeSpeaker("grok build")).toBeNull();
    expect(normalizeSpeaker("")).toBeNull();
  });

  it("preserves unknown names and labels an empty speaker honestly", () => {
    expect(displaySpeaker(" Nova ")).toBe("Nova");
    expect(displaySpeaker(" ")).toBe("Unknown agent");
  });

  it("aims speech at the speaker rather than request role", () => {
    expect(messageBubble(message({ speaker: "grok", recipient: "codex" })).tail).toBe(
      "toward-grok",
    );
    expect(messageBubble(message({ eventType: "response", speaker: "codex" })).tail).toBe(
      "toward-codex",
    );
  });

  it("keeps Parley errors out of robot speech", () => {
    const bubble = messageBubble(
      message({ eventType: "error", speaker: "codex", heading: "Parley execution error" }),
    );
    expect(bubble.kind).toBe("parley-error");
    expect(bubble.speaker).toBe("unknown");
    expect(bubble.tail).toBe("none");
    expect(bubble.displayName).toBe("Parley");
  });

  it("attaches pending text only to an exact known recipient", () => {
    expect(pendingBubble("Request logged; no response event yet", "grok").tail).toBe(
      "toward-grok",
    );
    expect(pendingBubble("Request logged; no response event yet", "").tail).toBe("none");
    expect(pendingBubble("Request logged; no response event yet", "other").speaker).toBe(
      "unknown",
    );
  });

  it("orders one newest exchange chronologically and preserves hostile text", () => {
    const bubbles = attributeExchange({
      request: message(),
      completion: message({
        eventKey: "event:2",
        eventId: "event-2",
        eventType: "response",
        heading: "Completion",
        speaker: "grok",
        recipient: "codex",
        excerpt: "done & exact",
      }),
      pendingLabel: null,
      completionHeading: "Completion",
    });
    expect(bubbles.map((bubble) => bubble.speaker)).toEqual(["codex", "grok"]);
    expect(bubbles[0]?.excerpt).toBe("<img src=x onerror=alert(1)>");
  });
});
