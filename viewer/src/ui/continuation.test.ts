/**
 * @vitest-environment happy-dom
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ContinuationEvidence, ContinuationStatus, ExchangeSummary } from "../contracts";
import type { ViewerApi } from "../ipc";

import {
  buildContinuationSection,
  isContinuationReceiptPresented,
  resolveContinuationReceiptEventKey,
} from "./continuation";

const ID = "123e4567-e89b-12d3-a456-426614174000";
const HASH = "a".repeat(64);

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("direct Grok continuation", () => {
  it("allows drafting but refuses sending when no task is published", async () => {
    const api = continuationApi(absentStatus());
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("No task is currently published");
    });
    expect(view.section.querySelector<HTMLDetailsElement>(".continuation-dock")?.open).toBe(false);
    expect(view.section.querySelector("summary")?.textContent).toContain(
      "No task is currently published",
    );
    expect(view.section.querySelector<HTMLTextAreaElement>("textarea")?.disabled).toBe(false);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(
      true,
    );
    view.stop();
  });

  it.each(["reviewed", "expired", "ambiguous", "running"])(
    "keeps the draft editable while sending is blocked in %s state",
    async (state) => {
      vi.useFakeTimers();
      const submit = vi.fn();
      const view = mount(continuationApi(armedStatus({ state }), { submitContinuation: submit }));
      await vi.advanceTimersByTimeAsync(0);
      view.section.querySelector<HTMLDetailsElement>(".continuation-dock")!.open = true;
      const textarea = view.section.querySelector<HTMLTextAreaElement>("textarea")!;
      expect(textarea.disabled).toBe(false);
      textarea.focus();
      textarea.value = "Keep this exact draft.\n🧬";
      textarea.setSelectionRange(5, 11);
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
      await vi.advanceTimersByTimeAsync(1_500);
      expect(document.activeElement).toBe(textarea);
      expect(textarea.value).toBe("Keep this exact draft.\n🧬");
      expect(textarea.selectionStart).toBe(5);
      expect(textarea.selectionEnd).toBe(11);
      expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(true);
      view.section.querySelector<HTMLFormElement>("form")!.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
      expect(submit).not.toHaveBeenCalled();
      view.stop();
      vi.useRealTimers();
    },
  );

  it("preserves draft and caret through task changes and failed status polls", async () => {
    vi.useFakeTimers();
    let next = armedStatus();
    let unavailable = false;
    const submit = vi.fn();
    const api = continuationApi(next, {
      getContinuationStatus: async () => {
        if (unavailable) {
          throw new Error("offline");
        }
        return next;
      },
      submitContinuation: submit,
    });
    const view = mount(api);
    await vi.advanceTimersByTimeAsync(0);
    view.section.querySelector<HTMLDetailsElement>(".continuation-dock")!.open = true;
    const textarea = view.section.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.focus();
    textarea.value = "Draft for the published task.";
    textarea.setSelectionRange(6, 9);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(false);

    unavailable = true;
    await vi.advanceTimersByTimeAsync(1_500);
    expect(textarea.disabled).toBe(false);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(true);
    view.section.querySelector<HTMLFormElement>("form")!.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    expect(submit).not.toHaveBeenCalled();

    unavailable = false;
    next = armedStatus({ state: "reviewed" });
    await vi.advanceTimersByTimeAsync(1_500);
    expect(textarea.disabled).toBe(false);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(true);
    next = armedStatus();
    await vi.advanceTimersByTimeAsync(1_500);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(false);
    expect(document.activeElement).toBe(textarea);
    expect(textarea.value).toBe("Draft for the published task.");
    expect(textarea.selectionStart).toBe(6);
    expect(textarea.selectionEnd).toBe(9);
    view.stop();
    vi.useRealTimers();
  });

  it("sends only the exact bounded message supplied by the user", async () => {
    const submit = vi.fn(async () => ({ continuationId: ID, state: "completed" }));
    const api = continuationApi(armedStatus(), { submitContinuation: submit });
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.section.querySelector<HTMLTextAreaElement>("textarea")?.disabled).toBe(false);
    });
    const textarea = view.section.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "Please finish the published tests.";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    view.section.querySelector<HTMLFormElement>("form")?.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await vi.waitFor(() => {
      expect(submit).toHaveBeenCalledWith("Please finish the published tests.");
    });
    view.stop();
  });

  it("keeps the next draft editable and retains it when submission fails", async () => {
    let rejectSubmission = (_error: Error): void => undefined;
    const submit = vi.fn(() => new Promise<{ continuationId: string; state: string }>((_resolve, reject) => {
      rejectSubmission = reject;
    }));
    const view = mount(continuationApi(armedStatus(), { submitContinuation: submit }));
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("Ready for a direct message");
    });
    view.section.querySelector<HTMLDetailsElement>(".continuation-dock")!.open = true;
    const textarea = view.section.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.focus();
    textarea.value = "Keep the exact submitted message.\n🧬";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    const form = view.section.querySelector<HTMLFormElement>("form")!;
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    expect(submit).toHaveBeenCalledTimes(1);
    expect(submit).toHaveBeenCalledWith("Keep the exact submitted message.\n🧬");
    expect(textarea.disabled).toBe(false);
    expect(textarea.readOnly).toBe(false);
    expect(document.activeElement).toBe(textarea);
    textarea.value = "My next draft.";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));

    rejectSubmission(new Error("submission refused"));
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("Grok did not accept");
    });
    expect(textarea.readOnly).toBe(false);
    expect(textarea.value).toBe("My next draft.");
    expect(document.activeElement).toBe(textarea);
    view.stop();
  });

  it("ignores status reads from before submission and never clears a newer draft", async () => {
    vi.useFakeTimers();
    let completeSubmit = (): void => undefined;
    let completeOldPoll = (): void => undefined;
    let reads = 0;
    const api = continuationApi(armedStatus(), {
      getContinuationStatus: async () => {
        reads += 1;
        if (reads === 2) {
          return new Promise<ContinuationStatus>((resolve) => {
            completeOldPoll = () => resolve(armedStatus());
          });
        }
        return reads === 1 ? armedStatus() : armedStatus({ state: "running" });
      },
      submitContinuation: () => new Promise((resolve) => {
        completeSubmit = () => resolve({ continuationId: ID, state: "running" });
      }),
    });
    const view = mount(api);
    await vi.advanceTimersByTimeAsync(0);
    view.section.querySelector<HTMLDetailsElement>(".continuation-dock")!.open = true;
    const textarea = view.section.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "Exact submitted message.";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(1_500);
    view.section.querySelector<HTMLFormElement>("form")!.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    textarea.value = "Next unsent draft.";
    textarea.setSelectionRange(4, 8);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    completeSubmit();
    await vi.advanceTimersByTimeAsync(0);
    completeOldPoll();
    await vi.advanceTimersByTimeAsync(0);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(true);
    expect(textarea.value).toBe("Next unsent draft.");
    expect(textarea.selectionStart).toBe(4);
    expect(textarea.selectionEnd).toBe(8);
    await vi.advanceTimersByTimeAsync(1_500);
    expect(view.section.textContent).toContain("Grok is working");
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(true);
    view.stop();
  });

  it("keeps exact prompt and reply in the transcript and receipts only on request", async () => {
    const hostile = "<img src=x onerror=alert(1)> exact reply";
    const unreceived = completedStatus({ reply: hostile, userReceived: false });
    const received = completedStatus({ reply: hostile, userReceived: true });
    const receive = vi.fn(async () => received);
    const api = continuationApi(unreceived, { receiveContinuation: receive });
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.receiptCandidate()).toEqual({
        continuationId: ID,
        evidenceHash: HASH,
        completionEventId: "response-1",
      });
    });
    expect(view.section.textContent).not.toContain(hostile);
    expect(view.section.textContent).not.toContain("Finish the task.");
    expect(view.section.querySelector("img")).toBeNull();
    expect(receive).not.toHaveBeenCalled();
    view.recordVisibleReceipt(view.receiptCandidate()!);
    await vi.waitFor(() => {
      expect(receive).toHaveBeenCalledWith(ID, HASH);
      expect(view.section.textContent).toContain("Displayed and receipted");
    });
    view.stop();
  });

  it("makes only one receipt attempt for an evidence hash", async () => {
    const receive = vi.fn(async () => {
      throw new Error("offline");
    });
    const api = continuationApi(completedStatus({ userReceived: false }), {
      receiveContinuation: receive,
    });
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.receiptCandidate()).not.toBeNull();
    });
    const candidate = view.receiptCandidate()!;
    view.recordVisibleReceipt(candidate);
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("could not be recorded");
    });
    view.recordVisibleReceipt(candidate);
    expect(receive).toHaveBeenCalledTimes(1);
    view.stop();
  });

  it("retains bounded scope and validation metadata behind disclosure", async () => {
    const view = mount(continuationApi(completedStatus()));
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("src/bounded/file.rs");
    });
    const details = view.section.querySelector<HTMLDetailsElement>(".continuation-latest");
    expect(details?.open).toBe(false);
    expect(details?.textContent).toContain("Codex must run tests during review");
    view.stop();
  });

  it("cancels only the running continuation reported by Rust", async () => {
    const running = armedStatus({ state: "running", runningContinuationId: ID });
    const cancel = vi.fn(async () => armedStatus({ state: "completed" }));
    const api = continuationApi(running, { cancelContinuation: cancel });
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.section.querySelector<HTMLButtonElement>(".continuation-cancel")?.hidden).toBe(
        false,
      );
    });
    view.section.querySelector<HTMLButtonElement>(".continuation-cancel")?.click();
    await vi.waitFor(() => expect(cancel).toHaveBeenCalledWith(ID));
    view.stop();
  });
});

describe("continuation receipt presentation", () => {
  const candidate = {
    continuationId: ID,
    evidenceHash: HASH,
    completionEventId: "response-1",
  };

  it("binds a raw completion id to exactly one opaque event key", () => {
    expect(resolveContinuationReceiptEventKey([exchange("event-a")], candidate)).toBe("event-a");
    expect(
      resolveContinuationReceiptEventKey(
        [exchange("event-a"), exchange("event-b", "exchange-b")],
        candidate,
      ),
    ).toBeNull();
    expect(
      resolveContinuationReceiptEventKey(
        [exchange("event-a", "exchange-a", "different-response")],
        candidate,
      ),
    ).toBeNull();
  });

  it("requires latest view and visible terminal evidence", () => {
    const row = document.createElement("article");
    const scrollport = document.createElement("div");
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue(rect(76, 124));
    vi.spyOn(scrollport, "getBoundingClientRect").mockReturnValue(rect(0, 100));
    const base = {
      mode: "latest" as const,
      documentVisible: true,
      matchingEventKey: "event-a",
      modalEventKey: null,
      compactDrawerOpen: false,
      row,
      scrollport,
    };
    expect(isContinuationReceiptPresented(base)).toBe(true);
    expect(isContinuationReceiptPresented({ ...base, mode: "search" })).toBe(false);
    expect(isContinuationReceiptPresented({ ...base, documentVisible: false })).toBe(false);
    expect(isContinuationReceiptPresented({ ...base, row: null })).toBe(false);
  });

  it("accepts full visibility for short rows and an exact matching modal", () => {
    const row = document.createElement("article");
    const scrollport = document.createElement("div");
    vi.spyOn(row, "getBoundingClientRect").mockReturnValue(rect(90, 100));
    vi.spyOn(scrollport, "getBoundingClientRect").mockReturnValue(rect(0, 100));
    const base = {
      mode: "latest" as const,
      documentVisible: true,
      matchingEventKey: "event-a",
      modalEventKey: null,
      compactDrawerOpen: false,
      row,
      scrollport,
    };
    expect(isContinuationReceiptPresented(base)).toBe(true);
    expect(
      isContinuationReceiptPresented({
        ...base,
        row: null,
        modalEventKey: "event-a",
        compactDrawerOpen: true,
      }),
    ).toBe(true);
    expect(
      isContinuationReceiptPresented({
        ...base,
        row: null,
        modalEventKey: "event-b",
        compactDrawerOpen: true,
      }),
    ).toBe(false);
    expect(
      isContinuationReceiptPresented({
        ...base,
        compactDrawerOpen: true,
      }),
    ).toBe(false);
  });
});

function mount(api: ViewerApi) {
  const controller = buildContinuationSection(api);
  document.body.append(controller.section);
  controller.start();
  return controller;
}

function continuationApi(
  initial: ContinuationStatus,
  overrides: Partial<ViewerApi> = {},
): ViewerApi {
  let current = initial;
  return {
    getContinuationStatus: async () => current,
    submitContinuation: async () => ({ continuationId: ID, state: "completed" }),
    cancelContinuation: async () => current,
    receiveContinuation: async () => {
      current = completedStatus({ userReceived: true });
      return current;
    },
    ...overrides,
  } as ViewerApi;
}

function absentStatus(): ContinuationStatus {
  return {
    schemaVersion: 1,
    state: "absent",
    message: "No published continuation is available",
    manifestId: null,
    worktreeName: null,
    writablePaths: [],
    expiresAtMs: null,
    continuationCount: 0,
    runningContinuationId: null,
    latest: null,
  };
}

function armedStatus(overrides: Partial<ContinuationStatus> = {}): ContinuationStatus {
  return {
    ...absentStatus(),
    state: "armed",
    message: null,
    manifestId: ID,
    worktreeName: "task-worktree",
    writablePaths: ["src/bounded"],
    expiresAtMs: 1_900_000_000_000,
    ...overrides,
  };
}

function completedStatus(
  evidenceOverrides: Partial<ContinuationEvidence> = {},
): ContinuationStatus {
  return armedStatus({
    state: "completed",
    continuationCount: 1,
    latest: {
      schemaVersion: 1,
      continuationId: ID,
      manifestId: ID,
      state: "succeeded",
      prompt: "Finish the task.",
      reply: "Done.",
      error: null,
      exchangeId: "exchange-1",
      requestEventId: "request-1",
      completionEventId: "response-1",
      changedPaths: ["src/bounded/file.rs"],
      validationState: "not_executed",
      diffFingerprint: "b".repeat(64),
      evidenceHash: HASH,
      completedAtMs: 1_800_000_000_000,
      userReceived: false,
      ...evidenceOverrides,
    },
  });
}

function exchange(
  eventKey: string,
  exchangeKey = "exchange-a",
  eventId = "response-1",
): ExchangeSummary {
  return {
    exchangeKey,
    sessionKey: "session-a",
    exchangeId: exchangeKey,
    sessionId: "session-a",
    sourcePath: "synthetic.jsonl",
    timestampMs: 1_800_000_000_000,
    request: null,
    completion: {
      eventKey,
      eventId,
      eventType: "response",
      speaker: "grok",
      recipient: "codex",
      timestampMs: 1_800_000_000_000,
      status: "ok",
      excerpt: "Done.",
      excerptExtracted: false,
      contentLength: 5,
    },
    pendingLabel: null,
  };
}

function rect(top: number, bottom: number): DOMRect {
  return {
    x: 0,
    y: top,
    top,
    bottom,
    left: 0,
    right: 100,
    width: 100,
    height: bottom - top,
    toJSON: () => ({}),
  } as DOMRect;
}
