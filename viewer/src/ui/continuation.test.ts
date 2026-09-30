/**
 * @vitest-environment happy-dom
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ContinuationEvidence, ContinuationStatus } from "../contracts";
import type { ViewerApi } from "../ipc";

import { buildContinuationSection } from "./continuation";

const ID = "123e4567-e89b-12d3-a456-426614174000";
const HASH = "a".repeat(64);

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("direct Grok continuation", () => {
  it("fails closed when no task is published", async () => {
    const api = continuationApi(absentStatus());
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("No task is currently published");
    });
    expect(view.section.querySelector<HTMLTextAreaElement>("textarea")?.disabled).toBe(true);
    expect(view.section.querySelector<HTMLButtonElement>(".continuation-send")?.disabled).toBe(
      true,
    );
    view.stop();
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

  it("renders hostile reply text literally before recording receipt", async () => {
    const hostile = "<img src=x onerror=alert(1)> exact reply";
    const unreceived = completedStatus({ reply: hostile, userReceived: false });
    const received = completedStatus({ reply: hostile, userReceived: true });
    const receive = vi.fn(async () => received);
    const api = continuationApi(unreceived, { receiveContinuation: receive });
    const view = mount(api);
    await vi.waitFor(() => {
      expect(view.section.querySelector(".continuation-reply")?.textContent).toBe(hostile);
      expect(receive).toHaveBeenCalledWith(ID, HASH);
    });
    expect(view.section.querySelector("img")).toBeNull();
    await vi.waitFor(() => {
      expect(view.section.textContent).toContain("Displayed and receipted");
    });
    view.stop();
  });

  it("does not record receipt while the Activity panel is hidden", async () => {
    const receive = vi.fn(async () => completedStatus({ userReceived: true }));
    const api = continuationApi(completedStatus({ userReceived: false }), {
      receiveContinuation: receive,
    });
    const panel = document.createElement("div");
    panel.setAttribute("role", "tabpanel");
    panel.hidden = true;
    const controller = buildContinuationSection(api);
    panel.append(controller.section);
    document.body.append(panel);
    controller.start();
    await vi.waitFor(() => {
      expect(controller.section.querySelector(".continuation-reply")?.textContent).toBe("Done.");
    });
    expect(receive).not.toHaveBeenCalled();
    controller.stop();
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
