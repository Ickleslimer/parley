import { describe, expect, it } from "vitest";

import type { MessagePreview, WidgetSnapshot } from "../contracts";

import { exactEventBody, presentMessage, presentWidgetSnapshot } from "./excerpt";
import { EXTRACTED_TASK_LABEL, PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";

const preview = (overrides: Partial<MessagePreview> = {}): MessagePreview => ({
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
    expect(presented.route).toBe("codex \u2192 grok");
    expect(presented.timestamp).toBe("1970-01-01 00:00:00 UTC");
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

  it("presents completion, pending, and Parley error states from the snapshot", () => {
    const pending: WidgetSnapshot = {
      sessionId: "s-1",
      exchangeId: "ex-3",
      request: preview({ excerpt: "waiting" }),
      completion: null,
      pendingLabel: PENDING_LABEL,
    };
    const pendingView = presentWidgetSnapshot(pending, true);
    expect(pendingView.request?.excerpt).toBe("waiting");
    expect(pendingView.completion).toBeNull();
    expect(pendingView.pendingLabel).toBe(PENDING_LABEL);
    expect(pendingView.completionHeading).toBe(PENDING_LABEL);

    const failed: WidgetSnapshot = {
      sessionId: "s-1",
      exchangeId: "ex-2",
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
    const failedView = presentWidgetSnapshot(failed, true);
    expect(failedView.pendingLabel).toBeNull();
    expect(failedView.completion?.heading).toBe(PARLEY_ERROR_LABEL);
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
