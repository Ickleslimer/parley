import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { REQUIRED_FIXTURES, type FixtureRequest } from "./contracts";

const captureScript = readFileSync(
  fileURLToPath(new URL("../../scripts/capture-fixtures.ps1", import.meta.url)),
  "utf8",
);

describe("synthetic capture contract", () => {
  it("covers every frozen surface, state, viewport, tab, and text scale", () => {
    expect(REQUIRED_FIXTURES).toHaveLength(24);
    for (const fixture of REQUIRED_FIXTURES) {
      const query = [
        `view=${fixture.surface}`,
        `fixture=${fixture.scenario}`,
        fixture.inspectorTab ? `tab=${fixture.inspectorTab}` : null,
        fixture.textScale === 2 ? "text-scale=2" : null,
      ]
        .filter((part): part is string => part !== null)
        .join("&");
      expect(captureScript).toContain(`Query = "${query}"`);
      expect(captureScript).toContain(`Width = ${fixture.width}; Height = ${fixture.height}`);
      if (fixture.reducedMotion) {
        expect(captureScript).toContain(
          `Query = "${query}"; Width = ${fixture.width}; Height = ${fixture.height}; ReducedMotion = $true`,
        );
      }
    }

    const surface = REQUIRED_FIXTURES.filter((fixture) => fixture.surface === "widget-surface");
    expect(surface.map(describeFixture)).toEqual([
      "live 720x560 scale=1 motion=false",
      "paused-unread 720x560 scale=1 motion=false",
      "collapsed 720x560 scale=1 motion=false",
      "expanded 720x560 scale=1 motion=false",
      "pending 720x560 scale=1 motion=false",
      "completion 720x560 scale=1 motion=false",
      "error 720x560 scale=1 motion=false",
      "unknown-agent 720x560 scale=1 motion=false",
      "empty 720x560 scale=1 motion=false",
      "missing-image 720x560 scale=1 motion=false",
      "live 720x560 scale=1 motion=true",
      "live 720x560 scale=2 motion=false",
      "live 480x420 scale=1 motion=false",
      "live 960x720 scale=1 motion=false",
    ]);
  });

  it("uses isolated headless Edge and a bounded graceful server shutdown", () => {
    expect(captureScript).toContain('"--headless=new"');
    expect(captureScript).toContain('"--disable-background-networking"');
    expect(captureScript).not.toContain('"--hide-scrollbars"');
    expect(captureScript).toContain('"--user-data-dir=$edgeProfile"');
    expect(captureScript).toContain('"http://127.0.0.1:1420/__fixture_shutdown"');
    expect(captureScript).toContain("WaitForExit(5000)");
    expect(captureScript).toContain("-WindowStyle Hidden");
    expect(captureScript).not.toMatch(/WindowStyle\s+(?:Normal|Maximized|Minimized)/i);
  });
});

function describeFixture(fixture: FixtureRequest): string {
  return `${fixture.scenario} ${fixture.width}x${fixture.height} scale=${fixture.textScale} motion=${fixture.reducedMotion}`;
}
