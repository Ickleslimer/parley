import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const directory = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(directory, "../styles/studio-focus.css"), "utf8");
const tokens = readFileSync(join(directory, "../styles/tokens.css"), "utf8");

const FROZEN_REGIONS = [
  "studio-header",
  "studio-status-line",
  "studio-utilities",
  "studio-banner",
  "studio-exit",
  "studio-transcript",
  "studio-transcript-scroll",
  "studio-search",
  "studio-drawer",
];

describe("studio focus stylesheet", () => {
  it("scopes every rule beneath a frozen studio region", () => {
    const selectors = ruleSelectors(css);
    expect(selectors.length).toBeGreaterThan(10);
    for (const selector of selectors) {
      if (selector.startsWith("@")) {
        expect(selector).toContain("prefers-reduced-motion");
        continue;
      }
      expect(selector).toContain('[data-region="studio-');
      expect(FROZEN_REGIONS.some((region) => selector.includes(region))).toBe(true);
    }
  });

  it("describes the wide column, compact sheet, targets, focus, clamp, scroll, and sticky close", () => {
    expect(tokens).toContain("--target-min: 44px");
    expect(css).toContain('[data-region="studio-drawer"].is-wide');
    expect(css).toContain('[data-region="studio-drawer"].is-compact');
    expect(css).toContain('[data-region="studio-drawer"].is-closed');
    expect(css).toMatch(/position:\s*static/);
    expect(css).toMatch(/position:\s*fixed/);
    expect(css).toContain("min-width: var(--target-min)");
    expect(css).toContain("min-height: var(--target-min)");
    expect(css).toContain("outline: 3px solid var(--focus-on-paper)");
    expect(css).toContain("-webkit-line-clamp: 4");
    expect(css).toMatch(/\[data-region="studio-transcript-scroll"\][\s\S]*overflow:\s*auto/);
    expect(css).toMatch(/\[data-region="studio-drawer"\]\.is-wide,\s*\[data-region="studio-drawer"\]\.is-compact[\s\S]*overflow:\s*auto/);
    expect(css).toMatch(/\.studio-focus-close[\s\S]*position:\s*sticky/);
    expect(css).toContain("@media (prefers-reduced-motion: reduce)");
    expect(css).toContain("animation: none");
    expect(css).toContain("var(--paper)");
    expect(css).toContain("var(--radius-paper)");
  });

  it("rejects inspector, widget, continuation, and infinite motion selectors", () => {
    expect(css).not.toMatch(/studio-inspector/);
    expect(css).not.toMatch(/continuation-/);
    expect(css).not.toMatch(/widget-/);
    expect(css).not.toMatch(/@keyframes/);
    expect(css).not.toMatch(/infinite/i);
    expect(css).not.toMatch(/text-transform\s*:\s*uppercase/i);
  });
});

function ruleSelectors(stylesheet: string): string[] {
  const stripped = stylesheet.replace(/\/\*[\s\S]*?\*\//g, "");
  return Array.from(stripped.matchAll(/([^{}]+)\{/g), (match) => match[1]?.trim() ?? "").filter(
    (selector) => selector.length > 0,
  );
}
