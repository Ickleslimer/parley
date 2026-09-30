// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import { STUDIO_FOCUS_ROWS } from "../fixtures/studio-focus";
import { createStudioTranscript } from "./studio-transcript-view";
import type { TranscriptPaint } from "./studio-seam";

const source = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "studio-transcript-view.ts"),
  "utf8",
);

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("studio transcript view", () => {
  it("renders reverse-chronological synthetic rows with quiet dates and literal hostile text", () => {
    const view = mount();
    view.paint(sample());

    expect(
      Array.from(view.list.querySelectorAll(".studio-focus-date"), (node) => node.textContent),
    ).toEqual(["30 September 2026", "29 September 2026"]);
    expect(
      Array.from(view.list.querySelectorAll(".studio-focus-excerpt"), (node) => node.textContent),
    ).toEqual(STUDIO_FOCUS_ROWS.map((row) => row.excerpt));
    const excerpts = view.list.querySelectorAll(".studio-focus-excerpt");
    const hostile = excerpts.item(excerpts.length - 1);
    expect(hostile?.textContent).toBe(STUDIO_FOCUS_ROWS[3]?.excerpt);
    expect(hostile?.childNodes[0]?.nodeType).toBe(Node.TEXT_NODE);
    expect(view.region.querySelector("img, svg, script, canvas")).toBeNull();
    expect(view.region.textContent).not.toMatch(/event:|exchange:|eventId|session|\\\\|[A-Za-z]:\\/);
    expect(view.list.textContent).toContain("Codex");
    expect(view.list.textContent).toContain("Grok");
    expect(view.list.textContent).toContain("Unknown speaker");
    expect(view.list.textContent).toContain("Search result");
  });

  it("uses the local calendar day when a row has a machine timestamp label", () => {
    const view = mount();
    const localTimestamp = new Date(2026, 8, 30, 0, 15, 0).getTime();
    view.paint({
      ...sample(),
      rows: [
        {
          ...STUDIO_FOCUS_ROWS[0],
          timestampMs: localTimestamp,
          timeLabel: "2026-09-30 00:15:00",
        },
      ],
    });

    expect(view.list.querySelector(".studio-focus-date")?.textContent).toBe("30 September 2026");
  });

  it("keeps pending rows noninteractive and separates focus from activation", () => {
    const callbacks = handlers();
    const view = createStudioTranscript(callbacks);
    document.body.append(view.region);
    view.paint(sample());

    const pending = view.list.querySelector("[data-pending='true']");
    expect(pending?.tagName).toBe("DIV");
    expect(pending?.querySelector("button")).toBeNull();
    expect(view.list.querySelectorAll("button")).toHaveLength(3);
    key(pending, "Enter");
    key(pending, " ");
    expect(callbacks.onActivate).not.toHaveBeenCalled();
    expect(callbacks.onFocus).not.toHaveBeenCalled();

    const request = button(view.list, "event:newest-request");
    request.focus();
    key(request, "ArrowDown");
    expect(callbacks.onFocus.mock.calls).toEqual([["event:newest-completion"]]);
    expect(callbacks.onActivate).not.toHaveBeenCalled();

    callbacks.onFocus.mockClear();
    const completion = button(view.list, "event:newest-completion");
    key(completion, "End");
    expect(callbacks.onFocus.mock.calls).toEqual([["event:hostile-search"]]);
    expect(callbacks.onActivate).not.toHaveBeenCalled();

    callbacks.onFocus.mockClear();
    key(button(view.list, "event:hostile-search"), "Enter");
    expect(callbacks.onActivate).toHaveBeenCalledTimes(1);
    expect(callbacks.onActivate.mock.calls[0]).toEqual(["event:hostile-search", "exchange:hostile-search"]);
    expect(callbacks.onFocus).not.toHaveBeenCalled();

    callbacks.onActivate.mockClear();
    button(view.list, "event:newest-request").click();
    expect(callbacks.onActivate).toHaveBeenCalledTimes(1);
    expect(callbacks.onActivate.mock.calls[0]).toEqual(["event:newest-request", "exchange:newest"]);
  });

  it("shows jump and load-earlier controls without scrolling or focusing during paint", () => {
    const callbacks = handlers();
    const view = createStudioTranscript(callbacks);
    const outsider = document.createElement("button");
    document.body.append(view.region, outsider);
    outsider.focus();
    const focus = vi.spyOn(HTMLElement.prototype, "focus");
    const intoView = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => undefined);
    let scrollWrites = 0;
    Object.defineProperty(view.scrollport, "scrollTop", {
      configurable: true,
      get: () => 24,
      set: () => {
        scrollWrites += 1;
      },
    });

    view.searchInput.value = "kept query";
    view.paint({
      ...sample(),
      focusedEventKey: "event:newest-completion",
      selectedEventKey: "event:newest-request",
      hasEarlier: true,
      showJumpLatest: true,
      jumpLatestLabel: "Jump to latest",
      emptyMessage: null,
    });

    expect(scrollWrites).toBe(0);
    expect(focus).not.toHaveBeenCalled();
    expect(intoView).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(outsider);
    expect(button(view.list, "event:newest-completion").tabIndex).toBe(0);
    expect(button(view.list, "event:newest-request").tabIndex).toBe(-1);
    expect(button(view.list, "event:newest-request").getAttribute("aria-current")).toBe("true");
    expect(button(view.list, "event:newest-completion").hasAttribute("aria-current")).toBe(false);
    expect(view.scrollport.lastElementChild).toBe(view.loadEarlier);
    expect(view.loadEarlier.hidden).toBe(false);
    expect(view.jumpLatest.hidden).toBe(false);
    expect(view.jumpLatest.compareDocumentPosition(view.scrollport) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(view.searchInput.value).toBe("kept query");

    view.jumpLatest.click();
    view.loadEarlier.click();
    view.searchForm.requestSubmit();
    expect(callbacks.onJumpLatest).toHaveBeenCalledTimes(1);
    expect(callbacks.onLoadEarlier).toHaveBeenCalledTimes(1);
    expect(callbacks.onSearch.mock.calls).toEqual([["kept query"]]);
    expect(callbacks.onActivate).not.toHaveBeenCalled();

    view.paint({ ...sample(), showJumpLatest: false, hasEarlier: false, emptyMessage: "No messages yet" });
    expect(view.jumpLatest.hidden).toBe(true);
    expect(view.loadEarlier.hidden).toBe(true);
    expect(view.empty.hidden).toBe(false);
    expect(view.empty.textContent).toBe("No messages yet");
  });

  it("does not assign scroll or import the viewer command boundary", () => {
    expect(source).not.toMatch(/scrollTop|scrollIntoView/);
    expect(source).not.toMatch(/from ["']\.\.\/ipc["']/);
    expect(source).not.toMatch(/ViewerApi|innerHTML/);
  });
});

function mount() {
  const view = createStudioTranscript(handlers());
  document.body.append(view.region);
  return view;
}

function handlers() {
  return {
    onSearch: vi.fn(),
    onLoadEarlier: vi.fn(),
    onJumpLatest: vi.fn(),
    onFocus: vi.fn(),
    onActivate: vi.fn(),
  };
}

function sample(): TranscriptPaint {
  return {
    mode: "latest",
    rows: STUDIO_FOCUS_ROWS,
    selectedEventKey: null,
    focusedEventKey: null,
    meta: "Newest first",
    emptyMessage: null,
    hasEarlier: false,
    showJumpLatest: false,
    jumpLatestLabel: "Jump to latest",
  };
}

function button(list: ParentNode, eventKey: string): HTMLButtonElement {
  const found = list.querySelector<HTMLButtonElement>(`button[data-event-key="${eventKey}"]`);
  if (!found) {
    throw new Error(`Missing row ${eventKey}`);
  }
  return found;
}

function key(target: Element | null, value: string): void {
  target?.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, cancelable: true }));
}
