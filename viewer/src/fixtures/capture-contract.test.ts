import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { REQUIRED_FIXTURES } from "./contracts";

const captureScript = readFileSync(
  fileURLToPath(new URL("../../scripts/capture-fixtures.ps1", import.meta.url)),
  "utf8",
);

describe("synthetic capture contract", () => {
  it("covers every frozen surface, state, viewport, tab, and text scale", () => {
    expect(REQUIRED_FIXTURES).toHaveLength(25);
    for (const fixture of REQUIRED_FIXTURES) {
      const query = [
        `view=${fixture.surface}`,
        `fixture=${fixture.scenario}`,
        fixture.inspectorTab ? `tab=${fixture.inspectorTab}` : null,
        fixture.textScale === 2 ? "text-scale=2" : null,
        fixture.surface === "widget-surface" ? `fixture-width=${fixture.width}` : null,
        fixture.surface === "widget-surface" ? `fixture-height=${fixture.height}` : null,
      ]
        .filter((part): part is string => part !== null)
        .join("&");
      expect(captureScript).toContain(`Query = "${query}"`);
      expect(captureScript).toContain(`Width = ${fixture.width}; Height = ${fixture.height}`);
      if (fixture.reducedMotion) {
        expect(captureScript).toContain(`Query = "${query}"; Width = ${fixture.width}; Height = ${fixture.height}; ReducedMotion = $true`);
      }
    }
  });

  it("uses isolated headless Edge and a bounded graceful server shutdown", () => {
    expect(captureScript).toContain('"--headless=new"');
    expect(captureScript).toContain('"--disable-background-networking"');
    expect(captureScript).toContain('"--user-data-dir=$edgeProfile"');
    expect(captureScript).toContain('"http://127.0.0.1:1420/__fixture_shutdown"');
    expect(captureScript).toContain("WaitForExit(5000)");
    expect(captureScript).toContain("-WindowStyle Hidden");
    expect(captureScript).not.toMatch(/WindowStyle\s+(?:Normal|Maximized|Minimized)/i);
  });
});
