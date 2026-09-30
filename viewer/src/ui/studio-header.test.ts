// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import { createStudioHeader } from "./studio-header";
import type { StudioHeaderPaint } from "./studio-seam";

const HOSTILE = "<img src=x onerror=\"throw new Error('not text')\">";
const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "studio-header.ts"), "utf8");

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("studio header", () => {
  it("paints the title, one status line, an explicit banner, utilities, and exit", () => {
    const onExit = vi.fn();
    const onUtility = vi.fn();
    const header = createStudioHeader({ onExit, onUtility });
    document.body.append(header.region);
    header.paint(paint());

    expect(header.region.querySelector("h1")?.textContent).toBe("Conversation studio");
    expect(header.status.textContent).toBe("Watching 2 sources");
    expect(header.banner.hidden).toBe(false);
    expect(header.banner.textContent).toBe("Widget unavailable");
    expect(header.status.textContent).not.toContain("Widget unavailable");
    expect(header.status.textContent).not.toContain("unread");
    expect(header.utilities.button.textContent).toContain("Utilities");
    expect(header.utilities.button.textContent).toContain("2 unread, 1 pending");
    expect(header.exit.textContent).toBe("Exit");
    expect(header.region.querySelector("img")).toBeNull();

    header.exit.click();
    expect(onExit).toHaveBeenCalledTimes(1);
    header.utilities.button.click();
    header.utilities.items.settings.click();
    expect(onUtility.mock.calls).toEqual([["settings"]]);
  });

  it("keeps hostile banner text literal and removes the banner without rewriting status", () => {
    const header = createStudioHeader({ onExit: vi.fn(), onUtility: vi.fn() });
    document.body.append(header.region);
    header.paint({ ...paint(), banner: HOSTILE, unreadCount: 0, pendingCount: 0 });

    expect(header.banner.textContent).toBe(HOSTILE);
    expect(header.banner.querySelector("img")).toBeNull();
    expect(header.banner.childNodes[0]?.nodeType).toBe(Node.TEXT_NODE);
    expect(header.utilities.button.textContent).not.toContain("unread");
    expect(header.utilities.button.textContent).not.toContain("pending");

    const focus = vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(() => undefined);
    header.paint({ ...paint(), status: "Watching 1 source", banner: null, unreadCount: 0, pendingCount: 0 });
    expect(header.status.textContent).toBe("Watching 1 source");
    expect(header.banner.hidden).toBe(true);
    expect(header.banner.textContent).toBe("");
    expect(focus).not.toHaveBeenCalled();
  });

  it("does not import the viewer command boundary", () => {
    expect(source).not.toMatch(/from ["']\.\.\/ipc["']/);
    expect(source).not.toMatch(/ViewerApi|innerHTML/);
    expect(source).not.toMatch(/[A-Za-z]:\\+|\/Users\/|sessionId|eventId/);
  });
});

function paint(): StudioHeaderPaint {
  return {
    status: "Watching 2 sources",
    banner: "Widget unavailable",
    unreadCount: 2,
    pendingCount: 1,
  };
}
