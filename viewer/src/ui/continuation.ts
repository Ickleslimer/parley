import type { ContinuationEvidence, ContinuationStatus, ExchangeSummary } from "../contracts";
import type { ViewerApi } from "../ipc";

import { el, labelledControl, setText } from "./dom";
import { formatTimestamp } from "./format";
import { LANDMARKS } from "./landmarks";
import { createSingleFlightPoller } from "./poll";
import type { TranscriptMode } from "./studio-seam";

const MAX_MESSAGE_CHARS = 8_000;
const CONTINUATION_POLL_MS = 1_500;

export interface ContinuationController {
  section: HTMLElement;
  start: () => void;
  stop: () => void;
  receiptCandidate: () => ContinuationReceiptCandidate | null;
  recordVisibleReceipt: (candidate: ContinuationReceiptCandidate) => void;
}

export interface ContinuationReceiptCandidate {
  continuationId: string;
  evidenceHash: string;
  completionEventId: string;
}

export interface ContinuationSectionOptions {
  onStatusChange?: () => void;
}

export interface ContinuationReceiptPresentation {
  mode: TranscriptMode;
  documentVisible: boolean;
  matchingEventKey: string | null;
  modalEventKey: string | null;
  compactDrawerOpen: boolean;
  row: HTMLElement | null;
  scrollport: HTMLElement;
}

export function buildContinuationSection(
  api: ViewerApi,
  options: ContinuationSectionOptions = {},
): ContinuationController {
  let status: ContinuationStatus | null = null;
  let error: string | null = null;
  let submitPending = false;
  let controlPending = false;
  let stopped = false;
  let latestRevision: string | null = null;
  const receiptAttempts = new Set<string>();

  const availability = el("span", {
    className: "continuation-availability",
    attrs: { role: "status", "aria-live": "polite" },
  });
  const scope = el("p", { className: "continuation-scope" });
  const grants = el("ul", {
    className: "continuation-grants",
    attrs: { "aria-label": "Published writable paths" },
  });
  const actionError = el("p", {
    className: "continuation-error",
    attrs: { role: "status" },
  });
  actionError.hidden = true;

  const latestSummary = el("summary", { text: "Latest direct-turn evidence" });
  const latestMeta = el("p", { className: "continuation-meta" });
  const latestOutcome = el("p", { className: "continuation-meta" });
  const latestChanges = el("ul", {
    className: "continuation-changes",
    attrs: { "aria-label": "Changed paths reported by the continuation broker" },
  });
  const latestValidation = el("p", { className: "continuation-meta" });
  const latestReceipt = el("p", { className: "continuation-meta" });
  const latest = el("details", {
    className: "continuation-latest",
    children: [
      latestSummary,
      latestMeta,
      latestOutcome,
      latestChanges,
      latestValidation,
      latestReceipt,
    ],
  });
  latest.hidden = true;

  const textarea = el("textarea", {
    className: "continuation-input",
    attrs: {
      id: "continuation-message",
      rows: "6",
      maxlength: String(MAX_MESSAGE_CHARS),
      placeholder: "Tell Grok what to finish within the published task…",
    },
  });
  const characterCount = el("span", {
    className: "continuation-count",
    text: `0 / ${MAX_MESSAGE_CHARS}`,
  });
  const send = el("button", {
    className: "studio-action continuation-send",
    text: "Send to Grok",
    attrs: { type: "submit" },
  });
  const cancel = el("button", {
    className: "studio-action continuation-cancel",
    text: "Cancel direct turn",
    attrs: { type: "button" },
  });
  const form = el("form", {
    className: "continuation-form",
    children: [
      labelledControl("Message", textarea, "continuation-field"),
      el("div", {
        className: "continuation-form-footer",
        children: [characterCount, send, cancel],
      }),
    ],
  });

  const composer = el("details", {
    className: "continuation-dock",
    children: [
      el("summary", {
        children: [
          el("span", { className: "continuation-title", text: "Direct with Grok" }),
          availability,
        ],
      }),
      el("div", {
        className: "continuation-body",
        children: [
          el("p", {
            className: "continuation-caption",
            text: "Emergency continuation for the current published task. Grok keeps the same conversation and can edit only the paths Codex granted. Your receipt confirms display, not Codex review.",
          }),
          scope,
          grants,
          actionError,
          latest,
          form,
        ],
      }),
    ],
  });
  const section = el("section", {
    className: "panel panel-continuation",
    attrs: {
      "aria-label": "Direct with Grok",
      "data-region": LANDMARKS.studioComposer,
    },
    children: [composer],
  });

  const paint = (): void => {
    const state = status?.state ?? "loading";
    setText(availability, availabilityLabel(status, error));
    const hasManifest = status?.manifestId != null;
    scope.hidden = !hasManifest;
    grants.hidden = !hasManifest;
    setText(scope, scopeLabel(status));
    grants.replaceChildren(
      ...(status?.writablePaths ?? []).map((path) => el("li", { text: path })),
    );
    actionError.hidden = error == null;
    setText(actionError, error ?? "");

    const canSubmit = state === "armed" || state === "completed";
    textarea.disabled = !canSubmit || submitPending;
    send.disabled = !canSubmit || submitPending || textarea.value.trim().length === 0;
    cancel.hidden = state !== "running";
    cancel.disabled = state !== "running" || controlPending || !status?.runningContinuationId;

    paintLatest(status?.latest ?? null);
  };

  const paintLatest = (evidence: ContinuationEvidence | null): void => {
    if (!evidence) {
      latest.hidden = true;
      latestRevision = null;
      return;
    }
    latest.hidden = false;
    const revision = `${evidence.evidenceHash}:${evidence.userReceived}`;
    if (revision !== latestRevision) {
      latestRevision = revision;
      setText(
        latestMeta,
        `${resultLabel(evidence.state)} · ${formatTimestamp(evidence.completedAtMs)}`,
      );
      setText(
        latestOutcome,
        evidence.error != null
          ? "The exact failure record is available in the transcript."
          : "The exact reply is available in the transcript.",
      );
      latestChanges.hidden = evidence.changedPaths.length === 0;
      latestChanges.replaceChildren(
        ...evidence.changedPaths.map((path) => el("li", { text: path })),
      );
      setText(
        latestValidation,
        evidence.validationState === "not_executed"
          ? "The broker did not execute repository code. Codex must run tests during review."
          : "Validation state is unavailable; Codex review remains required.",
      );
      setText(
        latestReceipt,
        evidence.userReceived
          ? "Displayed and receipted by the user interface. Codex review is still separate."
          : "Receipt waits until the matching terminal message is visibly presented.",
      );
    }
  };

  const receiptCandidate = (): ContinuationReceiptCandidate | null => {
    const evidence = status?.latest;
    if (!evidence || evidence.userReceived || !evidence.completionEventId) {
      return null;
    }
    return {
      continuationId: evidence.continuationId,
      evidenceHash: evidence.evidenceHash,
      completionEventId: evidence.completionEventId,
    };
  };

  const recordVisibleReceipt = (candidate: ContinuationReceiptCandidate): void => {
    const current = receiptCandidate();
    if (
      !current ||
      current.continuationId !== candidate.continuationId ||
      current.evidenceHash !== candidate.evidenceHash ||
      current.completionEventId !== candidate.completionEventId ||
      receiptAttempts.has(candidate.evidenceHash)
    ) {
      return;
    }
    receiptAttempts.add(candidate.evidenceHash);
    void api
      .receiveContinuation(candidate.continuationId, candidate.evidenceHash)
      .then((next) => {
        if (stopped) {
          return;
        }
        status = next;
        error = null;
        paint();
        options.onStatusChange?.();
      })
      .catch(() => {
        if (stopped) {
          return;
        }
        error = "The reply is visible, but its user receipt could not be recorded.";
        paint();
        options.onStatusChange?.();
      });
  };

  const refresh = async (): Promise<void> => {
    try {
      const next = await api.getContinuationStatus();
      if (stopped) {
        return;
      }
      status = next;
      error = null;
    } catch {
      if (stopped) {
        return;
      }
      error = "Direct continuation status is unavailable.";
    }
    paint();
    options.onStatusChange?.();
  };

  textarea.addEventListener("input", () => {
    setText(characterCount, `${Array.from(textarea.value).length} / ${MAX_MESSAGE_CHARS}`);
    paint();
  });
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (submitPending || textarea.disabled || textarea.value.trim().length === 0) {
      return;
    }
    const text = textarea.value;
    submitPending = true;
    error = null;
    paint();
    void api
      .submitContinuation(text)
      .then(async () => {
        if (stopped) {
          return;
        }
        textarea.value = "";
        setText(characterCount, `0 / ${MAX_MESSAGE_CHARS}`);
        await refresh();
      })
      .catch(() => {
        if (!stopped) {
          error = "Grok did not accept the direct continuation request.";
        }
      })
      .finally(() => {
        if (!stopped) {
          submitPending = false;
          paint();
        }
      });
  });
  cancel.addEventListener("click", () => {
    const continuationId = status?.runningContinuationId;
    if (!continuationId || controlPending) {
      return;
    }
    controlPending = true;
    error = null;
    paint();
    void api
      .cancelContinuation(continuationId)
      .then((next) => {
        if (!stopped) {
          status = next;
        }
      })
      .catch(() => {
        if (!stopped) {
          error = "The direct continuation could not be cancelled.";
        }
      })
      .finally(() => {
        if (!stopped) {
          controlPending = false;
          paint();
        }
      });
  });

  const poller = createSingleFlightPoller(refresh, CONTINUATION_POLL_MS);
  paint();
  return {
    section,
    start: () => poller.start(),
    stop: () => {
      stopped = true;
      poller.stop();
    },
    receiptCandidate,
    recordVisibleReceipt,
  };
}

export function resolveContinuationReceiptEventKey(
  exchanges: readonly ExchangeSummary[],
  candidate: ContinuationReceiptCandidate,
): string | null {
  const matches = exchanges
    .map((exchange) => exchange.completion)
    .filter(
      (completion) =>
        completion != null &&
        completion.eventType !== "request" &&
        completion.eventId === candidate.completionEventId,
    );
  return matches.length === 1 ? (matches[0]?.eventKey ?? null) : null;
}

export function isContinuationReceiptPresented(
  presentation: ContinuationReceiptPresentation,
): boolean {
  if (
    presentation.mode !== "latest" ||
    !presentation.documentVisible ||
    !presentation.matchingEventKey
  ) {
    return false;
  }
  if (presentation.modalEventKey === presentation.matchingEventKey) {
    return true;
  }
  if (presentation.compactDrawerOpen) {
    return false;
  }
  if (!presentation.row) {
    return false;
  }
  const rowRect = presentation.row.getBoundingClientRect();
  const scrollRect = presentation.scrollport.getBoundingClientRect();
  if (rowRect.height <= 0 || rowRect.width <= 0) {
    return false;
  }
  const visibleHeight = Math.max(
    0,
    Math.min(rowRect.bottom, scrollRect.bottom) - Math.max(rowRect.top, scrollRect.top),
  );
  const visibleWidth = Math.max(
    0,
    Math.min(rowRect.right, scrollRect.right) - Math.max(rowRect.left, scrollRect.left),
  );
  return visibleWidth > 0 && visibleHeight >= Math.min(24, rowRect.height);
}

function availabilityLabel(status: ContinuationStatus | null, error: string | null): string {
  if (error) {
    return error;
  }
  if (!status) {
    return "Checking for a published continuation…";
  }
  switch (status.state) {
    case "armed":
      return "Ready for a direct message to Grok.";
    case "running":
      return "Grok is working on your direct message.";
    case "completed":
      return "The latest direct turn is complete; Codex review remains pending.";
    case "reviewed":
      return "Codex reviewed and closed this published continuation.";
    case "expired":
      return "The published continuation expired. Codex must publish a fresh one.";
    case "ambiguous":
      return "More than one task is published. Direct continuation is disabled.";
    case "absent":
      return "No task is currently published for direct continuation.";
    default:
      return status.message ?? `Continuation state: ${status.state}.`;
  }
}

function scopeLabel(status: ContinuationStatus | null): string {
  if (!status?.manifestId) {
    return "";
  }
  const expiry = status.expiresAtMs == null ? "unknown expiry" : formatTimestamp(status.expiresAtMs);
  return `${status.worktreeName ?? "Published worktree"} · available until ${expiry}`;
}

function resultLabel(state: string): string {
  return state
    .split("_")
    .filter(Boolean)
    .map((part, index) => (index === 0 ? `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}` : part))
    .join(" ");
}
