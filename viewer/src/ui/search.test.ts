import { describe, expect, it } from "vitest";

import { applyPageResult } from "./paging";
import {
  createSearchState,
  isSearchActive,
  normalizeQuery,
  searchCanPage,
  searchPhase,
  searchPhaseLabel,
  searchSummary,
  submitSearch,
} from "./search";

describe("search state", () => {
  it("treats blank queries as idle and does not activate search", () => {
    expect(normalizeQuery("  task:  ")).toBe("task:");
    expect(isSearchActive("   ")).toBe(false);
    expect(searchPhase(createSearchState())).toBe("idle");
    expect(searchPhaseLabel("idle", 0)).toBe("Enter a query to search exact event content");
    const submitted = submitSearch(createSearchState(), "   ");
    expect(submitted.query).toBe("");
    expect(submitted.loading).toBe(false);
  });

  it("pages exact matches and reports empty or ready labels", () => {
    let state = submitSearch(createSearchState(20), "boom");
    expect(state.query).toBe("boom");
    expect(searchPhase(state)).toBe("loading");
    expect(searchPhaseLabel("loading", 0)).toBe("Searching\u2026");

    state = {
      ...state,
      loading: false,
      total: 0,
    };
    expect(searchPhase(state)).toBe("empty");
    expect(searchPhaseLabel("empty", 0)).toBe("No exact matches");

    state = {
      ...state,
      total: 41,
      paging: applyPageResult(state.paging, { nextCursor: 20, total: 41, itemCount: 20 }),
    };
    expect(searchPhase(state)).toBe("ready");
    expect(searchSummary(state)).toContain("41 exact matches");
    expect(searchSummary(state)).toContain("1\u201320 of 41");
    expect(searchCanPage(state)).toEqual({ next: true, previous: false });

    state = { ...state, error: "Unable to search event content", total: 0 };
    expect(searchPhase(state)).toBe("error");
    expect(searchPhaseLabel("error", 0)).toBe("Unable to search event content");
  });
});
