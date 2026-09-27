import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { ILLUSTRATIONS } from "../ui/assets";

const stylesDirectory = fileURLToPath(new URL("../styles", import.meta.url));
const illustrationDirectory = fileURLToPath(
  new URL("../assets/illustrations", import.meta.url),
);
const uiDirectory = fileURLToPath(new URL("../ui", import.meta.url));
const styles = readdirSync(stylesDirectory)
  .filter((name) => name.endsWith(".css"))
  .map((name) => readFileSync(`${stylesDirectory}/${name}`, "utf8"))
  .join("\n");
const productionUi = readdirSync(uiDirectory)
  .filter((name) => name.endsWith(".ts") && !name.endsWith(".test.ts"))
  .map((name) => readFileSync(`${uiDirectory}/${name}`, "utf8"))
  .join("\n");

describe("Two Chairs style contract", () => {
  it("keeps every required text and focus pair above its deterministic threshold", () => {
    expect(contrast("#241c16", "#f3e6d0")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#5c5146", "#f3e6d0")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#234e86", "#f3e6d0")).toBeGreaterThanOrEqual(3);
    expect(contrast("#f3e6d0", "#2c2428")).toBeGreaterThanOrEqual(3);
    expect(contrast("#4c3100", "#f0c983")).toBeGreaterThanOrEqual(4.5);
    expect(contrast("#521c17", "#e6aaa1")).toBeGreaterThanOrEqual(4.5);
  });

  it("keeps controls and focus indicators visibly sized", () => {
    expect(styles).toContain("--target-min: 44px");
    expect(styles).toMatch(/outline:\s*3px\s+solid\s+var\(--focus-on-paper\)/);
  });

  it("removes the old glow language and endless motion", () => {
    expect(styles).not.toContain("#7ee0ff");
    expect(styles).not.toMatch(/radial-gradient\([^)]*(?:cyan|purple|147\s+72\s+255|17\s+211\s+255)/i);
    expect(styles).not.toMatch(/animation(?:-iteration-count)?\s*:[^;]*(?:infinite|Infinity)/i);
    expect(styles).not.toMatch(/text-transform\s*:\s*uppercase/i);
    expect(styles).toContain("@media (prefers-reduced-motion: reduce)");
  });

  it("keeps literal production labels in sentence case", () => {
    expect(productionUi).not.toMatch(
      /(?:text|placeholder|"aria-label"):\s*"(?=[^"]{3,}")[A-Z][A-Z0-9 /_-]*"/,
    );
  });

  it("keeps the shipped illustration payload below 650 KiB", () => {
    const total = readdirSync(illustrationDirectory).reduce(
      (sum, name) => sum + statSync(`${illustrationDirectory}/${name}`).size,
      0,
    );
    expect(total).toBeLessThanOrEqual(650 * 1024);
  });

  it("keeps illustration metadata tied to the shipped bytes", () => {
    const files = {
      plate: "workshop-plate.webp",
      codex: "codex-robot.webp",
      grok: "grok-robot.webp",
    } as const;

    for (const [key, name] of Object.entries(files)) {
      const bytes = readFileSync(`${illustrationDirectory}/${name}`);
      const metadata = ILLUSTRATIONS[key as keyof typeof ILLUSTRATIONS];
      expect(metadata.byteLength).toBe(bytes.byteLength);
      expect(metadata.sha256).toBe(createHash("sha256").update(bytes).digest("hex"));
    }
  });
});

function contrast(foreground: string, background: string): number {
  const [lighter, darker] = [luminance(foreground), luminance(background)].sort(
    (left, right) => right - left,
  );
  return ((lighter ?? 0) + 0.05) / ((darker ?? 0) + 0.05);
}

function luminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)
    ?.map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
    );
  if (!channels || channels.length !== 3) {
    throw new Error(`Invalid color: ${hex}`);
  }
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}
