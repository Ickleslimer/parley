import { describe, expect, it } from "vitest";

import type { ViewerStatus } from "../contracts";

import {
  applyPageResult,
  canGoNext,
  canGoPrevious,
  clampPageLimit,
  dataRefreshPlan,
  PAGE_LIMIT_MAX,
  PAGE_LIMIT_MIN,
  pageRangeLabel,
  requestNextPage,
  requestPreviousPage,
  resetPaging,
} from "./paging";

const status = (overrides: Partial<ViewerStatus> = {}): ViewerStatus => ({
  sourceState: "watching",
  generation: 1,
  bytesRead: 10,
  sessionCount: 2,
  exchangeCount: 4,
  lastEventTimestampMs: 8,
  trayAvailable: true,
  underlayState: "attached",
  widgetVisible: true,
  diagnostics: {
    malformedLines: 0,
    oversizedLines: 0,
    unsupportedRecords: 0,
    duplicateEvents: 0,
    ioErrors: 0,
    aliasCollisions: 0,
    lastError: null,
  },
  sources: [
    {
      path: "C:\\\\a.jsonl",
      identity: "c:\\\\a.jsonl",
      sourceState: "watching",
      generation: 1,
      bytesRead: 10,
      sessionCount: 2,
      exchangeCount: 4,
      lastEventTimestampMs: 8,
      diagnostics: {
        malformedLines: 0,
        oversizedLines: 0,
        unsupportedRecords: 0,
        duplicateEvents: 0,
        ioErrors: 0,
        aliasCollisions: 0,
        lastError: null,
      },
      aliasOf: null,
    },
  ],
  ...overrides,
});

describe("paging state", () => {
  it("clamps page limits to the event-engine range", () => {
    expect(clampPageLimit(0)).toBe(PAGE_LIMIT_MIN);
    expect(clampPageLimit(201)).toBe(PAGE_LIMIT_MAX);
    expect(clampPageLimit(20.9)).toBe(20);
    expect(clampPageLimit(Number.NaN)).toBe(20);
  });

  it("walks forward and back using cursors without inventing pages", () => {
    let page = applyPageResult(resetPaging(20), {
      nextCursor: 20,
      total: 57,
      itemCount: 20,
    });
    expect(pageRangeLabel(page)).toBe("1\u201320 of 57");
    expect(canGoNext(page)).toBe(true);
    expect(canGoPrevious(page)).toBe(false);

    const next = requestNextPage(page);
    expect(next).not.toBeNull();
    page = applyPageResult(next!, {
      nextCursor: 40,
      total: 57,
      itemCount: 20,
    });
    expect(page.cursor).toBe(20);
    expect(pageRangeLabel(page)).toBe("21\u201340 of 57");
    expect(canGoPrevious(page)).toBe(true);

    const previous = requestPreviousPage(page);
    expect(previous?.cursor).toBeNull();
    expect(requestNextPage(applyPageResult(resetPaging(20), { nextCursor: null, total: 3, itemCount: 3 }))).toBeNull();
    expect(requestPreviousPage(resetPaging(20))).toBeNull();
    expect(pageRangeLabel(resetPaging(20))).toBe("0 of 0");
  });

  it("resets lists on generation or source changes and refreshes on new events", () => {
    const current = status();
    expect(dataRefreshPlan(null, current)).toBe("refresh");
    expect(dataRefreshPlan(current, current)).toBe("none");
    expect(dataRefreshPlan(current, status({ exchangeCount: 5 }))).toBe("refresh");
    expect(dataRefreshPlan(current, status({ bytesRead: 18 }))).toBe("refresh");
    expect(dataRefreshPlan(current, status({ generation: 2 }))).toBe("reset");
    expect(
      dataRefreshPlan(
        current,
        status({
          sources: [
            {
              ...current.sources[0]!,
              path: "C:\\\\b.jsonl",
              identity: "c:\\\\b.jsonl",
            },
          ],
        }),
      ),
    ).toBe("reset");
  });
});
