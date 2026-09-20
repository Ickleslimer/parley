import type { ViewerStatus } from "../contracts";

export const PAGE_LIMIT = 20;
export const PAGE_LIMIT_MIN = 1;
export const PAGE_LIMIT_MAX = 200;

export interface PagingState {
  cursor: number | null;
  nextCursor: number | null;
  previousCursors: Array<number | null>;
  total: number;
  itemCount: number;
  limit: number;
}

export type DataRefreshPlan = "reset" | "refresh" | "none";

export function clampPageLimit(limit: number): number {
  if (!Number.isFinite(limit)) {
    return PAGE_LIMIT;
  }
  return Math.min(PAGE_LIMIT_MAX, Math.max(PAGE_LIMIT_MIN, Math.trunc(limit)));
}

export function resetPaging(limit = PAGE_LIMIT): PagingState {
  return {
    cursor: null,
    nextCursor: null,
    previousCursors: [],
    total: 0,
    itemCount: 0,
    limit: clampPageLimit(limit),
  };
}

export function applyPageResult(
  state: PagingState,
  result: { nextCursor: number | null; total: number; itemCount: number },
): PagingState {
  return {
    ...state,
    nextCursor: result.nextCursor,
    total: result.total,
    itemCount: result.itemCount,
  };
}

export function requestNextPage(state: PagingState): PagingState | null {
  if (state.nextCursor == null) {
    return null;
  }
  return {
    ...state,
    previousCursors: [...state.previousCursors, state.cursor],
    cursor: state.nextCursor,
  };
}

export function requestPreviousPage(state: PagingState): PagingState | null {
  if (state.previousCursors.length === 0) {
    return null;
  }
  const previousCursors = state.previousCursors.slice(0, -1);
  const cursor = state.previousCursors[state.previousCursors.length - 1] ?? null;
  return {
    ...state,
    previousCursors,
    cursor,
  };
}

export function canGoNext(state: PagingState): boolean {
  return state.nextCursor != null;
}

export function canGoPrevious(state: PagingState): boolean {
  return state.previousCursors.length > 0;
}

export function pageRangeLabel(state: PagingState): string {
  if (state.total === 0 || state.itemCount === 0) {
    return `0 of ${state.total}`;
  }
  const start = (state.cursor ?? 0) + 1;
  const end = (state.cursor ?? 0) + state.itemCount;
  return `${start}\u2013${end} of ${state.total}`;
}

export function dataRefreshPlan(
  previous: ViewerStatus | null,
  current: ViewerStatus,
): DataRefreshPlan {
  if (!previous) {
    return "refresh";
  }
  if (previous.generation !== current.generation || previous.sourcePath !== current.sourcePath) {
    return "reset";
  }
  if (
    previous.bytesRead !== current.bytesRead ||
    previous.sessionCount !== current.sessionCount ||
    previous.exchangeCount !== current.exchangeCount ||
    previous.lastEventTimestampMs !== current.lastEventTimestampMs ||
    previous.sourceState !== current.sourceState
  ) {
    return "refresh";
  }
  return "none";
}
