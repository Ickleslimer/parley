// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { createStudioDrawer } from "./studio-drawer-chrome";

const source = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "studio-drawer-chrome.ts"),
  "utf8",
);

describe("studio drawer chrome", () => {
  it("contains long exact text without shrinking drawer controls or disclosures", () => {
    const drawer = createStudioDrawer();
    const styles = document.createElement("style");
    const directory = dirname(fileURLToPath(import.meta.url));
    styles.textContent = ["studio.css", "studio-focus.css"]
      .map((name) => readFileSync(join(directory, "../styles", name), "utf8"))
      .join("\n");
    const body = "Exact synthetic <script>text</script> 🧪\n".repeat(2_000);
    drawer.exactBody.textContent = body;
    drawer.setLayoutClass("wide");
    document.head.append(styles);
    document.body.append(drawer.region);
    try {
      const exactStyle = getComputedStyle(drawer.exactBody);
      expect(exactStyle.overflow).toBe("auto");
      expect(exactStyle.flexGrow).toBe("1");
      expect(exactStyle.flexShrink).toBe("1");
      expect(exactStyle.flexBasis).toBe("auto");
      expect(drawer.exactBody.textContent).toBe(body);
      expect(drawer.exactBody.querySelector("script")).toBeNull();
      drawer.setLayoutClass("compact");
      expect(getComputedStyle(drawer.exactBody).overflow).toBe("auto");
    } finally {
      styles.remove();
      drawer.region.remove();
    }
  });

  it("starts closed with one exact body, one collapsed disclosure, and a utility host", () => {
    const drawer = createStudioDrawer();

    expect(drawer.region.classList.contains("is-closed")).toBe(true);
    expect(drawer.region.classList.contains("is-wide")).toBe(false);
    expect(drawer.region.classList.contains("is-compact")).toBe(false);
    expect(drawer.close.textContent).toBe("Close");
    expect(drawer.region.querySelectorAll("pre")).toHaveLength(1);
    expect(drawer.exactBody).toBe(drawer.region.querySelector("pre"));
    expect(drawer.exactBody.textContent).toBe("");
    expect(drawer.region.querySelectorAll("details")).toHaveLength(1);
    expect(drawer.technicalDetails.open).toBe(false);
    expect(drawer.technicalDetails.querySelector("summary")?.textContent).toBe("Technical details");
    expect(drawer.technicalContent.parentElement).toBe(drawer.technicalDetails);
    expect(drawer.utilityHost.childNodes).toHaveLength(0);
    expect(drawer.region.querySelector("img")).toBeNull();
  });

  it("toggles only the closed, wide, and compact classes", () => {
    const drawer = createStudioDrawer();
    drawer.region.classList.add("kept");

    drawer.setLayoutClass("wide");
    expect(classNames(drawer.region)).toContain("is-wide");
    expect(classNames(drawer.region)).not.toContain("is-closed");
    expect(classNames(drawer.region)).not.toContain("is-compact");

    drawer.setLayoutClass("compact");
    expect(classNames(drawer.region)).toContain("is-compact");
    expect(classNames(drawer.region)).not.toContain("is-wide");

    drawer.setLayoutClass("closed");
    expect(classNames(drawer.region)).toContain("is-closed");
    expect(classNames(drawer.region)).toContain("kept");
    expect(classNames(drawer.region)).toContain("studio-focus-drawer");
  });

  it("does not own modal, inert, hidden, or receipt behavior", () => {
    const drawer = createStudioDrawer();
    drawer.setLayoutClass("compact");

    expect(drawer.region.hasAttribute("hidden")).toBe(false);
    expect(drawer.region.hasAttribute("inert")).toBe(false);
    expect(drawer.region.getAttribute("role")).toBeNull();
    expect(drawer.region.getAttribute("aria-modal")).toBeNull();
    expect(drawer.region.textContent).not.toMatch(/receipt|eventId|session|\\\\|\/Users\//);
    expect(source).not.toMatch(/\bhidden\b|\binert\b|aria-modal|\brole\b|receipt|innerHTML|ViewerApi/);
    expect(source).not.toMatch(/from ["']\.\.\/ipc["']/);
  });
});

function classNames(region: HTMLElement): string[] {
  return Array.from(region.classList);
}
