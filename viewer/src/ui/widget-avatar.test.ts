// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { AVATAR_ILLUSTRATIONS } from "./assets";
import { createWidgetAvatar, type WidgetAvatarAgent, type WidgetAvatarState } from "./widget-avatar";

const HOSTILE_SPEAKERS = [
  "Codex",
  "Grok",
  "CODEX",
  "GROK",
  " codex",
  "codex ",
  " grok ",
  "codex\n",
  "codex\u0000",
  "\u0441odex",
  "codex\u200b",
  "codex-device-working",
  "codexIdle",
  "codexWorking",
  "grokWorking",
  "unknown",
  "Parley",
  "Nova",
  "neutral ",
  "",
  "<img src=x onerror=alert(1)>",
  '"><img src=x onerror=alert(1)>',
  `codex ${AVATAR_ILLUSTRATIONS.grokWorking.path}`,
];

const css = stripComments(readFileSync(resolve("src/styles/avatar.css"), "utf8"));
const source = readFileSync(resolve("src/ui/widget-avatar.ts"), "utf8");

describe("widget avatar", () => {
  it("keeps hostile speaker-like strings from changing the closed agent selection", () => {
    for (const speaker of HOSTILE_SPEAKERS) {
      const node = createWidgetAvatar({ agent: speaker as WidgetAvatarAgent, working: true });
      expect(node.dataset.agent, speaker).toBe("neutral");
      expect(node.dataset.working, speaker).toBe("false");
      expect(node.querySelector("img"), speaker).toBeNull();
      expect(node.querySelector(".widget-avatar-working"), speaker).toBeNull();
      expect(node.querySelector(".widget-feed-device"), speaker).not.toBeNull();
      expect(node.textContent, speaker).toBe("");
      if (speaker.length > 0) {
        expect(node.outerHTML, speaker).not.toContain(speaker);
      }
    }

    const named = createWidgetAvatar("Codex" as unknown as WidgetAvatarState);
    expect(named.dataset.agent).toBe("neutral");
    expect(named.querySelector("img")).toBeNull();

    const coerced = createWidgetAvatar({
      agent: "codex",
      working: "true" as unknown as boolean,
      speaker: "grok",
      src: AVATAR_ILLUSTRATIONS.grokWorking.path,
    } as WidgetAvatarState);
    expect(coerced.dataset.agent).toBe("codex");
    expect(coerced.dataset.working).toBe("false");
    expect(coerced.querySelector("img")?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.codexIdle.path);
    expect(source).not.toContain("normalizeSpeaker");
    expect(source).not.toMatch(/\b(?:innerHTML|insertAdjacentHTML|matchMedia|requestAnimationFrame|setInterval)\b/);
  });

  it("selects the exact Codex and Grok idle and working assets", () => {
    expect(asset("codex", false)).toBe(AVATAR_ILLUSTRATIONS.codexIdle.path);
    expect(asset("codex", true)).toBe(AVATAR_ILLUSTRATIONS.codexWorking.path);
    expect(asset("grok", false)).toBe(AVATAR_ILLUSTRATIONS.grokIdle.path);
    expect(asset("grok", true)).toBe(AVATAR_ILLUSTRATIONS.grokWorking.path);

    for (const agent of ["codex", "grok"] as const) {
      for (const working of [false, true]) {
        const node = createWidgetAvatar({ agent, working });
        const images = node.querySelectorAll("img");
        expect(images).toHaveLength(1);
        expect(node.querySelector(".widget-feed-device")).toBeNull();
        expect(images[0]?.getAttribute("alt")).toBe("");
        expect(images[0]?.getAttribute("draggable")).toBe("false");
        expect(images[0]?.draggable).toBe(false);
        expect(node.className).toBe("widget-feed-avatar-slot");
        expect(node.dataset.agent).toBe(agent);
        expect(node.dataset.working).toBe(working ? "true" : "false");
        expect(node.getAttribute("aria-hidden")).toBe("true");
      }
    }
  });

  it("mirrors only Grok toward the conversation column", () => {
    const rules = topLevelRules(css);
    const mirror = rules.find(
      (rule) => rule.prelude === '.widget-feed-avatar-slot[data-agent="grok"]',
    );
    expect(mirror?.body).toMatch(/transform\s*:\s*scaleX\(-1\)/);
    expect(declarations(mirror?.body ?? "")).toEqual(["transform"]);
    expect(css).not.toMatch(/\[data-agent="codex"\][^{]*\{[^}]*scaleX\(-1\)/s);
  });

  it("renders neutral as a decorative device with no image", () => {
    for (const working of [false, true]) {
      const node = createWidgetAvatar({ agent: "neutral", working });
      expect(node.className).toBe("widget-feed-avatar-slot");
      expect(node.dataset.agent).toBe("neutral");
      expect(node.dataset.working).toBe("false");
      expect(node.getAttribute("aria-hidden")).toBe("true");
      expect(node.getAttribute("aria-live")).toBeNull();
      expect(node.getAttribute("tabindex")).toBeNull();
      expect(node.querySelector("img, svg, canvas, video, audio, button, a, input")).toBeNull();
      expect(node.querySelector(".widget-avatar-working")).toBeNull();
      const device = node.querySelector(".widget-feed-device");
      expect(device).not.toBeNull();
      expect(device?.className).toBe("widget-feed-device");
      expect(device?.querySelector(".widget-feed-device-screen")).not.toBeNull();
      expect(node.textContent).toBe("");
      expect(node.outerHTML).not.toMatch(/codex|grok/i);
    }
  });

  it("hides only the failed image and leaves surrounding text and controls in place", () => {
    const codex = createWidgetAvatar({ agent: "codex", working: true });
    const grok = createWidgetAvatar({ agent: "grok", working: false });
    const body = document.createElement("p");
    body.className = "widget-feed-body";
    body.textContent = "Ship the record";
    const control = document.createElement("button");
    control.type = "button";
    control.textContent = "Show full message";
    const row = document.createElement("div");
    row.append(codex, body, control, grok);
    document.body.append(row);

    const image = codex.querySelector("img");
    expect(image).not.toBeNull();
    image?.dispatchEvent(new Event("error"));
    image?.dispatchEvent(new Event("error"));

    expect(image?.classList.contains("is-missing")).toBe(true);
    expect(image?.isConnected).toBe(true);
    expect(image?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.codexWorking.path);
    expect(codex.dataset.agent).toBe("codex");
    expect(codex.querySelectorAll("img")).toHaveLength(1);
    expect(codex.querySelector(".widget-feed-device")).toBeNull();
    expect(body.textContent).toBe("Ship the record");
    expect(body.isConnected).toBe(true);
    expect(control.isConnected).toBe(true);
    expect(control.disabled).toBe(false);
    expect(control.textContent).toBe("Show full message");
    expect(grok.querySelector("img")?.classList.contains("is-missing")).toBe(false);
    expect(grok.querySelector("img")?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.grokIdle.path);
    expect(row.querySelector("script")).toBeNull();
  });

  it("gives the working state the only infinite animation", () => {
    expect(createWidgetAvatar({ agent: "codex", working: true }).querySelectorAll(".widget-avatar-working")).toHaveLength(1);
    expect(createWidgetAvatar({ agent: "grok", working: true }).querySelector(".widget-avatar-working")).not.toBeNull();
    expect(createWidgetAvatar({ agent: "codex", working: false }).querySelector(".widget-avatar-working")).toBeNull();
    expect(createWidgetAvatar({ agent: "grok", working: false }).querySelector(".widget-avatar-working")).toBeNull();
    expect(createWidgetAvatar({ agent: "neutral", working: true }).querySelector(".widget-avatar-working")).toBeNull();

    const rules = topLevelRules(css);
    const infiniteRules = rules.filter((rule) => rule.body.includes("infinite"));
    expect(infiniteRules).toHaveLength(1);
    const motion = infiniteRules[0];
    expect(motion?.prelude).toContain(".widget-avatar-working");
    expect(motion?.prelude).toContain('[data-working="true"]');
    expect(motion?.prelude).toContain('[data-agent="codex"]');
    expect(motion?.prelude).toContain('[data-agent="grok"]');
    expect(motion?.prelude).not.toContain("neutral");
    expect(motion?.prelude).not.toContain('[data-working="false"]');
    const motionProperties = declarations(motion?.body ?? "");
    expect(motionProperties.length).toBeGreaterThan(0);
    expect(motionProperties.every((property) => property.startsWith("animation"))).toBe(true);
    expect(css.match(/infinite/gi)).toHaveLength(1);
  });

  it("changes only transform and opacity inside keyframes", () => {
    const frames = topLevelRules(css).filter((rule) => rule.prelude.startsWith("@keyframes"));
    expect(frames).toHaveLength(1);
    expect(frames[0]?.prelude).toBe("@keyframes widget-avatar-working");
    const properties = declarations(frames[0]?.body ?? "");
    expect(properties.length).toBeGreaterThan(0);
    expect([...new Set(properties)].sort()).toEqual(["opacity", "transform"]);
  });

  it("disables the loop under reduced motion and keeps the static working asset", () => {
    const reduced = topLevelRules(css).find((rule) => rule.prelude.includes("prefers-reduced-motion"));
    expect(reduced?.prelude).toMatch(/prefers-reduced-motion:\s*reduce/);
    expect(reduced?.body).toMatch(/animation\s*:\s*none/);
    expect(reduced?.body).not.toMatch(/infinite/);
    expect(reduced?.body).not.toMatch(/display\s*:\s*none/);
    expect(css).not.toMatch(/--codex|--grok/);

    const working = createWidgetAvatar({ agent: "grok", working: true });
    expect(working.querySelector("img")?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.grokWorking.path);
    expect(source).not.toContain("matchMedia");
  });
});

function asset(agent: "codex" | "grok", working: boolean): string | null {
  return createWidgetAvatar({ agent, working }).querySelector("img")?.getAttribute("src") ?? null;
}

function stripComments(sheet: string): string {
  return sheet.replace(/\/\*[\s\S]*?\*\//g, "");
}

function topLevelRules(sheet: string): Array<{ prelude: string; body: string }> {
  const rules: Array<{ prelude: string; body: string }> = [];
  let cursor = 0;
  while (cursor < sheet.length) {
    while (cursor < sheet.length && /\s/.test(sheet[cursor] ?? "")) {
      cursor += 1;
    }
    if (cursor >= sheet.length) {
      break;
    }
    const open = sheet.indexOf("{", cursor);
    if (open === -1) {
      break;
    }
    const block = balanced(sheet, open);
    rules.push({ prelude: sheet.slice(cursor, open).trim(), body: block.inner });
    cursor = block.end;
  }
  return rules;
}

function balanced(text: string, open: number): { inner: string; end: number } {
  let depth = 0;
  for (let index = open; index < text.length; index += 1) {
    if (text[index] === "{") {
      depth += 1;
    } else if (text[index] === "}") {
      depth -= 1;
      if (depth === 0) {
        return { inner: text.slice(open + 1, index), end: index + 1 };
      }
    }
  }
  throw new Error("Unbalanced CSS");
}

function declarations(block: string): string[] {
  return [...block.matchAll(/(?:^|[;{}])\s*([A-Za-z-]+)\s*:/g)].map((match) => match[1]!.toLowerCase());
}
