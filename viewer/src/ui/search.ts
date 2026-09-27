import { canGoNext, canGoPrevious, pageRangeLabel, type PagingState, resetPaging } from "./paging";

export type SearchPhase = "idle" | "loading" | "empty" | "ready" | "error";

export interface SearchViewState {
  input: string;
  query: string;
  paging: PagingState;
  total: number;
  loading: boolean;
  error: string | null;
}

export function normalizeQuery(query: string): string {
  return query.trim();
}

export function isSearchActive(query: string): boolean {
  return normalizeQuery(query).length > 0;
}

export function createSearchState(limit?: number): SearchViewState {
  return {
    input: "",
    query: "",
    paging: resetPaging(limit),
    total: 0,
    loading: false,
    error: null,
  };
}

export function submitSearch(state: SearchViewState, input: string, limit?: number): SearchViewState {
  const query = normalizeQuery(input);
  return {
    input,
    query,
    paging: resetPaging(limit ?? state.paging.limit),
    total: 0,
    loading: query.length > 0,
    error: null,
  };
}

export function searchPhase(state: Pick<SearchViewState, "query" | "loading" | "total" | "error">): SearchPhase {
  if (!isSearchActive(state.query)) {
    return "idle";
  }
  if (state.loading) {
    return "loading";
  }
  if (state.error) {
    return "error";
  }
  if (state.total === 0) {
    return "empty";
  }
  return "ready";
}

export function searchPhaseLabel(phase: SearchPhase, total: number): string {
  switch (phase) {
    case "idle":
      return "Enter a query to search exact event content";
    case "loading":
      return "Searching\u2026";
    case "empty":
      return "No exact matches";
    case "ready":
      return `${total} exact match${total === 1 ? "" : "es"}`;
    case "error":
      return "Unable to search event content";
  }
}

export function searchSummary(state: SearchViewState): string {
  const phase = searchPhase(state);
  if (phase === "ready") {
    return `${searchPhaseLabel(phase, state.total)}; ${pageRangeLabel(state.paging)}`;
  }
  return searchPhaseLabel(phase, state.total);
}

export function searchCanPage(state: SearchViewState): { next: boolean; previous: boolean } {
  if (searchPhase(state) !== "ready") {
    return { next: false, previous: false };
  }
  return {
    next: canGoNext(state.paging),
    previous: canGoPrevious(state.paging),
  };
}
