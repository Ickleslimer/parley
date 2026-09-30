// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import { createStudioMenu } from "./studio-menu";

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "studio-menu.ts"), "utf8");

afterEach(() => {
  document.body.replaceChildren();
});

describe("studio utilities menu", () => {
  it("moves focus with arrows, home, and end without choosing", () => {
    const onChoose = vi.fn();
    const menu = createStudioMenu(onChoose);
    document.body.append(menu.button, menu.menu);

    menu.button.focus();
    key(menu.button, "ArrowDown");
    expect(document.activeElement).toBe(menu.items.activity);
    expect(menu.menu.hidden).toBe(false);

    key(menu.items.activity, "ArrowDown");
    expect(document.activeElement).toBe(menu.items.sources);
    key(menu.items.sources, "End");
    expect(document.activeElement).toBe(menu.items.settings);
    key(menu.items.settings, "Home");
    expect(document.activeElement).toBe(menu.items.activity);
    key(menu.items.activity, "ArrowUp");
    expect(document.activeElement).toBe(menu.items.activity);
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("chooses one utility id on enter or space and restores the button on escape", () => {
    const onChoose = vi.fn();
    const menu = createStudioMenu(onChoose);
    document.body.append(menu.button, menu.menu);
    menu.button.focus();
    key(menu.button, "ArrowDown");
    key(menu.items.activity, "ArrowDown");
    key(menu.items.sources, "Enter");

    expect(onChoose).toHaveBeenCalledTimes(1);
    expect(onChoose.mock.calls[0]).toEqual(["sources"]);
    expect(menu.menu.hidden).toBe(true);
    expect(document.activeElement).toBe(menu.button);

    menu.button.click();
    expect(document.activeElement).toBe(menu.items.activity);
    key(menu.items.activity, " ");
    expect(onChoose).toHaveBeenCalledTimes(2);
    expect(onChoose.mock.calls[1]).toEqual(["activity"]);

    menu.button.click();
    key(menu.items.activity, "Escape");
    expect(onChoose).toHaveBeenCalledTimes(2);
    expect(menu.menu.hidden).toBe(true);
    expect(document.activeElement).toBe(menu.button);
  });

  it("closes from outside without moving focus unless restoration is requested", () => {
    const menu = createStudioMenu(vi.fn());
    const other = document.createElement("button");
    document.body.append(menu.button, menu.menu, other);
    menu.button.click();
    other.focus();
    menu.close(false);
    expect(menu.menu.hidden).toBe(true);
    expect(document.activeElement).toBe(other);
    menu.close(true);
    expect(document.activeElement).toBe(menu.button);
  });

  it("does not import the viewer command boundary or render utility actions", () => {
    expect(source).not.toMatch(/from ["']\.\.\/ipc["']/);
    expect(source).not.toMatch(/ViewerApi|innerHTML|scrollIntoView/);
  });
});

function key(target: HTMLElement, value: string): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true }));
}
