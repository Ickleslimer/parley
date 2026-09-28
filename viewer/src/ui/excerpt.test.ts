import { describe, expect, it } from "vitest";

import type { MessagePreview } from "../contracts";

import {
  exactEventBody,
  participantInitial,
  presentExchange,
  presentMessage,
} from "./excerpt";
import { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";

const preview = (overrides: Partial<MessagePreview> = {}): MessagePreview => ({
  eventKey: "key-event-1",
  eventId: "event-1",
  eventType: "request",
  speaker: "codex",
  recipient: "grok",
  timestampMs: 0,
  status: "started",
  excerpt: "hello",
  excerptExtracted: false,
  contentLength: 5,
  ...overrides,
});

describe("exact excerpt presentation", () => {
  it("preserves the exact excerpt including leading space and HTML characters", () => {
    const presented = presentMessage(
      preview({
        excerpt: " Ship the engine <b>not html</b>",
        excerptExtracted: true,
        contentLength: 31,
      }),
      "request",
      true,
    );
    expect(presented.excerpt).toBe(" Ship the engine <b>not html</b>");
    expect(presented.excerpt).not.toContain("<span");
    expect(presented.extracted).toBe(true);
    expect(presented.extractedLabel).toBe(EXTRACTED_TASK_LABEL);
    expect(presented.heading).toBe("Request");
    expect(presented.speaker).toBe("codex");
    expect(presented.recipient).toBe("grok");
    expect(presented.initial).toBe("C");
    expect(presented.route).toBe("codex \u2192 grok");
    expect(presented.timestamp).toBe("1970-01-01 00:00:00 UTC");
  });

  it("exposes speaker and recipient fields without parsing the formatted route", () => {
    const presented = presentMessage(
      preview({ speaker: "cursor", recipient: "qwen", excerpt: "exact ask" }),
      "request",
      true,
    );
    expect(presented.speaker).toBe("cursor");
    expect(presented.recipient).toBe("qwen");
    expect(presented.initial).toBe("C");
    expect(presented.route).toBe("cursor \u2192 qwen");
    expect(presented.excerpt).toBe("exact ask");
  });

  it("treats a completion as the opposing participant from the event identities", () => {
    const presented = presentMessage(
      preview({
        eventId: "resp-9",
        eventType: "response",
        speaker: "gemini",
        recipient: "aider",
        excerpt: "exact reply",
        status: "completed",
        contentLength: 11,
      }),
      "completion",
      true,
    );
    expect(presented.heading).toBe("Completion");
    expect(presented.speaker).toBe("gemini");
    expect(presented.recipient).toBe("aider");
    expect(presented.initial).toBe("G");
    expect(presented.excerpt).toBe("exact reply");
  });

  it("derives avatar initials from any participant name", () => {
    expect(participantInitial("codex")).toBe("C");
    expect(participantInitial("grok")).toBe("G");
    expect(participantInitial("amazon_q")).toBe("A");
    expect(participantInitial("  qwen")).toBe("Q");
    expect(participantInitial("")).toBe("?");
  });

  it("does not treat a prefix excerpt as an extracted task", () => {
    const presented = presentMessage(
      preview({ excerpt: "task: should stay a prefix only", excerptExtracted: false }),
      "request",
      true,
    );
    expect(presented.excerpt).toBe("task: should stay a prefix only");
    expect(presented.extracted).toBe(false);
    expect(presented.extractedLabel).toBeNull();
  });

  it("presents completion, pending, and Parley error states from an exchange", () => {
    const pending = {
      request: preview({ excerpt: "waiting" }),
      completion: null,
      pendingLabel: PENDING_LABEL,
    };
    const pendingView = presentExchange(pending, true);
    expect(pendingView.request?.excerpt).toBe("waiting");
    expect(pendingView.completion).toBeNull();
    expect(pendingView.pendingLabel).toBe(PENDING_LABEL);
    expect(pendingView.completionHeading).toBe(PENDING_LABEL);

    const failed = {
      request: preview({ excerpt: "second" }),
      completion: preview({
        eventId: "err-2",
        eventType: "error",
        speaker: "codex",
        recipient: "grok",
        excerpt: "boom",
        status: "failed",
        excerptExtracted: false,
        contentLength: 4,
      }),
      pendingLabel: null,
    };
    const failedView = presentExchange(failed, true);
    expect(failedView.pendingLabel).toBeNull();
    expect(failedView.completion?.heading).toBe(PARLEY_ERROR_LABEL);
    expect(failedView.completion?.speaker).toBe("codex");
    expect(failedView.completion?.recipient).toBe("grok");
    expect(failedView.completion?.excerpt).toBe("boom");
    expect(failedView.completionHeading).toBe(PARLEY_ERROR_LABEL);
  });

  it("uses the exact execution error when an error has no content body", () => {
    expect(
      exactEventBody({ eventType: "error", content: "", error: "process timed out" }),
    ).toBe("process timed out");
    expect(
      exactEventBody({ eventType: "response", content: "exact reply", error: null }),
    ).toBe("exact reply");
  });
});
