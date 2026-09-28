// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import type { ViewerStatus, WidgetFeedExchange, WidgetFeedMessage, WidgetFeedPage } from "../contracts";
import { createSyntheticFixture } from "../fixtures/synthetic-api";
import type { ViewerApi } from "../ipc";

import { AVATAR_ILLUSTRATIONS } from "./assets";
import { LANDMARKS } from "./landmarks";
import { PARLEY_ERROR_LABEL, PENDING_LABEL } from "./labels";
import { assignProgrammaticScrollTop, mountWidgetFeed } from "./widget-feed";

const HOSTILE = "<img src=x onerror=alert(1)><script>alert(1)</script>";
const css = readFileSync(resolve("src/styles/scene.css"), "utf8");
const feedSource = readFileSync(resolve("src/ui/widget-feed.ts"), "utf8");
const surfaceSource = readFileSync(resolve("src/ui/widget-surface.ts"), "utf8");

afterEach(() => {
  document.body.replaceChildren();
  document.documentElement.removeAttribute("data-synthetic-fixture");
  window.history.replaceState(null, "", "/");
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("widget feed", () => {
  it("renders hostile text as text and keeps one polite region outside the scrollport", async () => {
    const mounted = mountFeed({
      items: [exchange(1, { requestBody: HOSTILE, contextOmitted: true })],
    });
    const body = await bodyFor(mounted.root, "request-1");
    expect(body.textContent).toBe(HOSTILE);
    expect(body.childNodes[0]?.nodeType).toBe(Node.TEXT_NODE);
    expect(body.querySelector("img, script")).toBeNull();
    expect(mounted.root.querySelector("script")).toBeNull();
    expect(mounted.root.textContent).toContain("Earlier context omitted");
    const status = mounted.root.querySelector(`#${LANDMARKS.widgetFeedStatus}`);
    const scroll = mounted.root.querySelector(`#${LANDMARKS.widgetFeedScroll}`);
    expect(scroll?.contains(status)).toBe(false);
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(mounted.root.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    expect(scroll?.contains(body)).toBe(true);
    expect(feedSource.includes("innerHTML")).toBe(false);
    expect(feedSource.includes("insertAdjacentHTML")).toBe(false);
    expect(surfaceSource.includes("innerHTML")).toBe(false);
    expect(feedSource.includes("codexWorking")).toBe(false);
    expect(feedSource.includes("grokWorking")).toBe(false);
    mounted.stop();
  });

  it("renders global page order and a quiet divider only when the session changes", async () => {
    const mounted = mountFeed({
      items: [
        exchange(2, { sessionKey: "session-z", timestampMs: 10, requestBody: "second on the page" }),
        exchange(1, { sessionKey: "session-a", timestampMs: 90, requestBody: "first on the page" }),
        exchange(3, { sessionKey: "session-a", timestampMs: 5, requestBody: "still the first session" }),
      ],
    });
    await bodyFor(mounted.root, "request-2");
    expect(exchangeKeys(mounted.root)).toEqual(["exchange-2", "exchange-1", "exchange-3"]);
    const dividers = [...mounted.root.querySelectorAll(".widget-feed-divider")];
    expect(dividers).toHaveLength(1);
    expect(dividers[0]?.textContent).toBe("New conversation");
    expect(dividers[0]?.textContent).not.toContain("session-");
    expect(mounted.root.textContent).not.toContain("session-z");
    mounted.stop();
  });

  it("pauses only for a user scroll beyond 24px and ignores programmatic scrolling", async () => {
    const mounted = mountFeed({
      items: [exchange(1)],
      hasEarlier: true,
      nextBefore: "exchange-1",
    });
    await bodyFor(mounted.root, "request-1");
    const scroll = scrollport(mounted.root);
    const live = button(mounted.root, LANDMARKS.widgetFeedLiveToggle);
    stubMetrics(scroll, { scrollHeight: 500, clientHeight: 100 });
    expect(live.getAttribute("aria-pressed")).toBe("true");

    assignProgrammaticScrollTop(scroll, 0);
    expect(live.getAttribute("aria-pressed")).toBe("true");

    scroll.scrollTop = 380;
    scroll.dispatchEvent(new Event("scroll"));
    expect(distance(scroll)).toBe(20);
    expect(live.getAttribute("aria-pressed")).toBe("true");

    scroll.scrollTop = 370;
    scroll.dispatchEvent(new Event("scroll"));
    expect(distance(scroll)).toBe(30);
    expect(live.getAttribute("aria-pressed")).toBe("false");
    expect(live.classList.contains("is-latched")).toBe(false);

    scroll.scrollTop = 400;
    scroll.dispatchEvent(new Event("scroll"));
    expect(distance(scroll)).toBe(0);
    expect(live.getAttribute("aria-pressed")).toBe("false");
    expect(mounted.root.querySelector(`#${LANDMARKS.widgetFeedJumpLive}`)?.hasAttribute("hidden")).toBe(true);

    live.click();
    await vi.waitFor(() => {
      expect(live.getAttribute("aria-pressed")).toBe("true");
    });
    expect(scroll.scrollTop).toBe(500);
    mounted.stop();
  });

  it("keeps the paused viewport and counts new request, response, and error keys", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let page = feedPage({
      items: [pendingExchange(1, "Waiting on the exact request")],
    });
    const getWidgetFeed = vi.fn(async () => page);
    const mounted = mountFeed({ getWidgetFeed });
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Waiting on the exact request");
    });
    const scroll = scrollport(mounted.root);
    stubMetrics(scroll, { scrollHeight: 800, clientHeight: 120 });
    scroll.scrollTop = 40;
    scroll.dispatchEvent(new Event("scroll"));
    expect(button(mounted.root, LANDMARKS.widgetFeedLiveToggle).getAttribute("aria-pressed")).toBe("false");

    page = feedPage({
      items: [
        {
          ...pendingExchange(1, "Waiting on the exact request"),
          pendingLabel: null,
          completion: message("completion-1", "response", "grok", "codex", "The reply arrived"),
        },
        exchange(2, { requestBody: "A newer request", responseBody: "A newer reply" }),
      ],
    });
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Jump to live \u00b7 3 new");
    });
    expect(scroll.scrollTop).toBe(40);
    expect(mounted.root.textContent).toContain("The reply arrived");
    expect(mounted.root.textContent).toContain("A newer request");
    expect(button(mounted.root, LANDMARKS.widgetFeedJumpLive).textContent).toBe("Jump to live \u00b7 3 new");
    mounted.stop();
  });

  it("fails closed when the history token changes and reloads only through jump to live", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const pages = [
      feedPage({ token: "history-a", items: [exchange(1, { requestBody: "alpha stays" })] }),
      feedPage({
        token: "history-b",
        items: [exchange(9, { requestBody: "beta must stay hidden" })],
      }),
      feedPage({ token: "history-b", items: [exchange(9, { requestBody: "beta is current" })] }),
    ];
    let index = 0;
    const getWidgetFeed = vi.fn(async () => pages[Math.min(index++, pages.length - 1)] ?? pages[0]!);
    const mounted = mountFeed({ getWidgetFeed });
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("alpha stays");
    });
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("History changed \u00b7 Jump to live");
    });
    expect(mounted.root.textContent).not.toContain("beta must stay hidden");
    expect(mounted.root.textContent).toContain("alpha stays");
    expect(button(mounted.root, LANDMARKS.widgetFeedLoadEarlier).disabled).toBe(true);

    button(mounted.root, LANDMARKS.widgetFeedJumpLive).click();
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("beta is current");
    });
    expect(mounted.root.textContent).not.toContain("History changed");
    expect(button(mounted.root, LANDMARKS.widgetFeedLiveToggle).getAttribute("aria-pressed")).toBe("true");
    expect(button(mounted.root, LANDMARKS.widgetFeedJumpLive).hidden).toBe(true);
    mounted.stop();
  });

  it("treats resetRequired as a closed history change", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const first = feedPage({ items: [exchange(1, { requestBody: "kept exact" })] });
    const reset = feedPage({
      resetRequired: true,
      items: [exchange(4, { requestBody: "reset hidden" })],
    });
    let call = 0;
    const mounted = mountFeed({
      getWidgetFeed: async () => {
        call += 1;
        return call === 1 ? first : reset;
      },
    });
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("kept exact");
    });
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("History changed \u00b7 Jump to live");
    });
    expect(mounted.root.textContent).not.toContain("reset hidden");
    mounted.stop();
  });

  it("anchors the viewport when earlier exchanges are prepended", async () => {
    const getWidgetFeed = vi.fn(async (before: string | null) => {
      if (before == null) {
        return feedPage({
          items: range(20, 40),
          hasEarlier: true,
          nextBefore: "exchange-20",
        });
      }
      return feedPage({
        items: range(0, 20),
        hasEarlier: false,
        nextBefore: null,
      });
    });
    const mounted = mountFeed({ getWidgetFeed });
    await vi.waitFor(() => {
      expect(exchangeKeys(mounted.root)).toHaveLength(20);
    });
    const scroll = scrollport(mounted.root);
    const metrics = { exchanges: () => mounted.root.querySelectorAll("[data-exchange-key]").length };
    Object.defineProperty(scroll, "scrollHeight", {
      configurable: true,
      get: () => 200 + metrics.exchanges() * 50,
    });
    Object.defineProperty(scroll, "clientHeight", { configurable: true, get: () => 100 });
    scroll.scrollTop = 15;
    const before = scroll.scrollTop;
    button(mounted.root, LANDMARKS.widgetFeedLoadEarlier).click();
    await vi.waitFor(() => {
      expect(exchangeKeys(mounted.root)[0]).toBe("exchange-0");
    });
    expect(scroll.scrollTop).toBe(before + 1000);
    expect(button(mounted.root, LANDMARKS.widgetFeedLiveToggle).getAttribute("aria-pressed")).toBe("true");
    expect(getWidgetFeed).toHaveBeenCalledWith("exchange-20");
    mounted.stop();
  });

  it("keeps at most 200 exchanges while loading earlier", async () => {
    const total = 240;
    const getWidgetFeed = vi.fn(async (before: string | null) => {
      const end = before == null ? total : Number(/^exchange-(\d+)$/.exec(before)?.[1] ?? total);
      const start = Math.max(0, end - 20);
      return feedPage({
        items: range(start, end),
        hasEarlier: start > 0,
        nextBefore: start > 0 ? `exchange-${start}` : null,
        totalExchanges: total,
      });
    });
    const mounted = mountFeed({ getWidgetFeed });
    await vi.waitFor(() => {
      expect(exchangeKeys(mounted.root)).toHaveLength(20);
    });
    for (let attempt = 0; attempt < 15; attempt += 1) {
      const control = button(mounted.root, LANDMARKS.widgetFeedLoadEarlier);
      if (control.disabled) {
        break;
      }
      const calls = getWidgetFeed.mock.calls.length;
      control.click();
      await vi.waitFor(() => {
        expect(getWidgetFeed.mock.calls.length).toBeGreaterThan(calls);
      });
    }
    const keys = exchangeKeys(mounted.root);
    expect(keys).toHaveLength(200);
    expect(keys[0]).toBe("exchange-0");
    expect(keys[199]).toBe("exchange-199");
    expect(keys).not.toContain("exchange-239");
    expect(button(mounted.root, LANDMARKS.widgetFeedLoadEarlier).disabled).toBe(true);
    mounted.stop();
  });

  it("shows the supplied prefix, expands the full message in place, and restores it", async () => {
    const prefix = Array.from({ length: 4_000 }, (_, index) => (index === 3_999 ? "\u00e9" : "a")).join("");
    expect(Array.from(prefix)).toHaveLength(4_000);
    const full = `${prefix} tail`;
    const getWidgetMessage = vi.fn(async (eventKey: string) =>
      eventKey === "response-1"
        ? message("response-1", "response", "grok", "codex", full, { truncated: false })
        : null,
    );
    const mounted = mountFeed({
      items: [
        exchange(1, {
          responseBody: prefix,
          truncated: true,
          fullCharacterLength: Array.from(full).length,
        }),
      ],
      getWidgetMessage,
    });
    const body = await bodyFor(mounted.root, "response-1");
    expect(body.textContent).toBe(prefix);
    const fullButton = row(mounted.root, "response-1").querySelector<HTMLButtonElement>(".widget-feed-full");
    expect(fullButton?.textContent).toBe("Show full message");
    fullButton?.click();
    await vi.waitFor(() => {
      expect(body.textContent).toBe(full);
    });
    expect(fullButton?.textContent).toBe("Collapse");
    expect(row(mounted.root, "response-1").querySelector(".widget-feed-body")).toBe(body);
    fullButton?.click();
    await vi.waitFor(() => {
      expect(body.textContent).toBe(prefix);
    });
    expect(getWidgetMessage).toHaveBeenCalledTimes(1);
    mounted.stop();
  });

  it("reports a missing full message without replacing the exact prefix", async () => {
    const mounted = mountFeed({
      items: [exchange(1, { responseBody: "exact prefix", truncated: true, fullCharacterLength: 80 })],
      getWidgetMessage: async () => null,
    });
    const body = await bodyFor(mounted.root, "response-1");
    row(mounted.root, "response-1").querySelector<HTMLButtonElement>(".widget-feed-full")?.click();
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Unable to load the full message");
    });
    expect(body.textContent).toBe("exact prefix");
    mounted.stop();
  });

  it("uses idle avatars, a neutral device, and Parley for errors and pending targets", async () => {
    const mounted = mountFeed({
      items: [
        {
          exchangeKey: "exchange-unknown",
          sessionKey: "session-a",
          timestampMs: 1,
          request: message("request-nova", "request", "Nova", "codex", "from nova"),
          completion: null,
          pendingLabel: null,
        },
        {
          exchangeKey: "exchange-error",
          sessionKey: "session-a",
          timestampMs: 2,
          request: message("request-codex", "request", "codex", "grok", "Ship the record"),
          completion: message("error-1", "error", "codex", "grok", "boom"),
          pendingLabel: null,
        },
        pendingExchange(3, PENDING_LABEL),
      ],
    });
    await bodyFor(mounted.root, "request-nova");
    const unknown = row(mounted.root, "request-nova");
    expect(unknown.dataset.speaker).toBe("unknown");
    expect(unknown.dataset.tail).toBe("none");
    expect(unknown.textContent).toContain("Nova");
    expect(unknown.textContent).toContain("from nova");
    expect(unknown.querySelector("img")).toBeNull();
    expect(unknown.querySelector(".widget-feed-device")).not.toBeNull();

    const failed = row(mounted.root, "error-1");
    expect(failed.dataset.kind).toBe("parley-error");
    expect(failed.dataset.tail).toBe("none");
    expect(failed.querySelector("img")).toBeNull();
    expect(failed.textContent).toContain(PARLEY_ERROR_LABEL);
    expect(failed.textContent).toContain("boom");
    expect(failed.textContent).toContain("Parley");

    const codex = row(mounted.root, "request-codex");
    expect(codex.dataset.tail).toBe("toward-codex");
    expect(codex.querySelector("img")?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.codexIdle.path);
    expect(codex.querySelector("img")?.getAttribute("alt")).toBe("");

    const pending = mounted.root.querySelector<HTMLElement>('[data-kind="pending"]');
    expect(pending?.dataset.tail).toBe("toward-grok");
    expect(pending?.textContent).toContain(PENDING_LABEL);
    expect(pending?.querySelector("img")?.getAttribute("src")).toBe(AVATAR_ILLUSTRATIONS.grokIdle.path);
    expect(mounted.root.querySelector("img")?.getAttribute("src")).not.toContain("working");
    mounted.stop();
  });

  it("withholds a mismatched projection instead of rendering its body", async () => {
    const mounted = mountFeed({
      items: [
        exchange(1, {
          requestBody: "secret withheld payload",
          projection: "withheld",
        }),
      ],
    });
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Message withheld");
    });
    expect(mounted.root.textContent).not.toContain("secret withheld payload");
    expect(row(mounted.root, "request-1").querySelector(".widget-feed-full")?.hasAttribute("hidden")).toBe(
      true,
    );
    mounted.stop();
  });

  it("opens the message event, or the newest completion, error, or request", async () => {
    const openWidgetEvent = vi.fn(async () => undefined);
    const completed = mountFeed({ items: [exchange(1)], openWidgetEvent });
    await bodyFor(completed.root, "response-1");
    row(completed.root, "request-1").querySelector<HTMLButtonElement>(".widget-feed-open-message")?.click();
    await vi.waitFor(() => {
      expect(openWidgetEvent).toHaveBeenCalledWith("request-1");
    });
    button(completed.root, LANDMARKS.widgetFeedOpenTranscript).click();
    await vi.waitFor(() => {
      expect(openWidgetEvent).toHaveBeenCalledWith("response-1");
    });
    completed.stop();

    const openError = vi.fn(async () => undefined);
    const failed = mountFeed({
      items: [
        {
          exchangeKey: "exchange-error",
          sessionKey: "session-a",
          timestampMs: 2,
          request: message("request-1", "request", "codex", "grok", "Ship the record"),
          completion: message("error-1", "error", "codex", "grok", "boom"),
          pendingLabel: null,
        },
      ],
      openWidgetEvent: openError,
    });
    await bodyFor(failed.root, "error-1");
    button(failed.root, LANDMARKS.widgetFeedOpenTranscript).click();
    await vi.waitFor(() => {
      expect(openError).toHaveBeenCalledWith("error-1");
    });
    failed.stop();

    const openPending = vi.fn(async () => undefined);
    const pending = mountFeed({
      items: [pendingExchange(1, PENDING_LABEL)],
      openWidgetEvent: openPending,
    });
    await vi.waitFor(() => {
      expect(pending.root.textContent).toContain(PENDING_LABEL);
    });
    button(pending.root, LANDMARKS.widgetFeedOpenTranscript).click();
    await vi.waitFor(() => {
      expect(openPending).toHaveBeenCalledWith("request-1");
    });
    pending.stop();
  });

  it("keeps controls at 44px, shows a scrollbar, and does not activate from the keyboard", async () => {
    const mounted = mountFeed({
      items: [exchange(1, { responseBody: "visible", truncated: true, fullCharacterLength: 20 })],
      hasEarlier: true,
      nextBefore: "exchange-0",
    });
    await bodyFor(mounted.root, "request-1");
    expect(css).toMatch(/\.widget-feed-load-earlier\s*\{[^}]*min-width:\s*44px;[^}]*min-height:\s*44px/);
    expect(css).toMatch(/\.widget-surface-button\.widget-feed-target\s*\{[^}]*min-width:\s*44px;[^}]*min-height:\s*44px/);
    expect(css).toMatch(/\.widget-feed-scroll\s*\{[^}]*overflow-y:\s*scroll/);
    expect(mounted.root.querySelector(`#${LANDMARKS.widgetSurfaceOlder}`)).toBeNull();
    expect(mounted.root.querySelector(`#${LANDMARKS.widgetSurfaceNewer}`)).toBeNull();
    const controls = [
      button(mounted.root, LANDMARKS.widgetFeedLoadEarlier),
      button(mounted.root, LANDMARKS.widgetFeedLiveToggle),
      button(mounted.root, LANDMARKS.widgetFeedOpenTranscript),
      ...mounted.root.querySelectorAll<HTMLButtonElement>(".widget-feed-full, .widget-feed-open-message"),
    ];
    for (const control of controls) {
      expect(control.classList.contains("widget-feed-target")).toBe(true);
      expect(control.tabIndex).toBe(-1);
      const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
      control.dispatchEvent(down);
      expect(down.defaultPrevented).toBe(true);
      control.focus();
      expect(document.activeElement).not.toBe(control);
      const key = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
      control.dispatchEvent(key);
      expect(key.defaultPrevented).toBe(true);
    }
    mounted.root.dispatchEvent(new WheelEvent("wheel", { deltaY: 80, bubbles: true }));
    expect(mounted.root.textContent).toContain("Following live");
    mounted.stop();
  });

  it("polls the newest page in a single flight", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    let active = 0;
    let maxActive = 0;
    const waiters: Array<() => void> = [];
    const getWidgetFeed = vi.fn(
      () =>
        new Promise<WidgetFeedPage>((resolve) => {
          active += 1;
          maxActive = Math.max(maxActive, active);
          waiters.push(() => {
            active -= 1;
            resolve(feedPage({ items: [exchange(1)] }));
          });
        }),
    );
    const mounted = mountFeed({ getWidgetFeed });
    await vi.waitFor(() => {
      expect(waiters).toHaveLength(1);
    });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(getWidgetFeed).toHaveBeenCalledTimes(1);
    expect(maxActive).toBe(1);
    waiters[0]?.();
    await vi.waitFor(() => {
      expect(mounted.root.textContent).toContain("Request 1");
    });
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(waiters).toHaveLength(2);
    });
    expect(maxActive).toBe(1);
    waiters[1]?.();
    mounted.stop();
  });

  it("keeps the same body node when an identical newest page arrives", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const page = feedPage({ items: [exchange(1, { requestBody: "Ship the engine" })] });
    const getWidgetFeed = vi.fn(async () => page);
    const mounted = mountFeed({ getWidgetFeed });
    const body = await bodyFor(mounted.root, "request-1");
    await vi.advanceTimersByTimeAsync(500);
    await vi.waitFor(() => {
      expect(getWidgetFeed.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
    expect(mounted.root.querySelector(".widget-feed-body")).toBe(body);
    mounted.stop();
  });

  it("builds the synthetic capture feeds for the lab chat states", async () => {
    const live = await createSyntheticFixture("live").api.getWidgetFeed(null);
    expect(live.items.map((item) => item.sessionKey)).toEqual([
      "session:synthetic-a",
      "session:synthetic-a",
      "session:synthetic-b",
    ]);
    expect(live.items[0]?.request?.contextOmitted).toBe(true);
    expect(live.items[0]?.request?.projection).toBe("current-request");
    expect(live.resetRequired).toBe(false);

    const collapsed = await createSyntheticFixture("collapsed").api.getWidgetFeed(null);
    const collapsedBody = collapsed.items[0]?.completion?.body ?? "";
    expect(Array.from(collapsedBody)).toHaveLength(4_000);
    expect(collapsed.items[0]?.completion?.truncated).toBe(true);
    const full = await createSyntheticFixture("collapsed").api.getWidgetMessage("event:completion-1");
    expect(full?.body.endsWith("end")).toBe(true);
    expect(Array.from(full?.body ?? "").length).toBeGreaterThan(4_000);

    const pending = await createSyntheticFixture("pending").api.getWidgetFeed(null);
    expect(pending.items[0]?.completion).toBeNull();
    expect(pending.items[0]?.pendingLabel).toBe(PENDING_LABEL);

    const failed = await createSyntheticFixture("error").api.getWidgetFeed(null);
    expect(failed.items[0]?.completion?.eventType).toBe("error");

    const unknown = await createSyntheticFixture("unknown-agent").api.getWidgetFeed(null);
    expect(unknown.items[0]?.request?.speaker).toBe("nova");

    const empty = await createSyntheticFixture("empty").api.getWidgetFeed(null);
    expect(empty.items).toEqual([]);

    const completion = await createSyntheticFixture("completion").api.getWidgetFeed(null);
    expect(completion.items[0]?.completion?.eventType).toBe("response");
    expect(completion.items[0]?.completion?.truncated).toBe(false);
  });

  it("seeds paused unread and expanded capture presentations", async () => {
    window.history.replaceState(null, "", "/?fixture=paused-unread");
    const paused = mountFeed({
      api: createSyntheticFixture("paused-unread").api,
    });
    await vi.waitFor(() => {
      expect(paused.root.textContent).toContain("Jump to live \u00b7 4 new");
    });
    expect(button(paused.root, LANDMARKS.widgetFeedLiveToggle).getAttribute("aria-pressed")).toBe("false");
    paused.stop();

    window.history.replaceState(null, "", "/?fixture=expanded");
    const expanded = mountFeed({ api: createSyntheticFixture("expanded").api });
    await vi.waitFor(() => {
      expect(
        expanded.root.querySelector('[data-event-key="event:completion-1"] .widget-feed-full')?.textContent,
      ).toBe("Collapse");
    });
    expect(expanded.root.textContent).toContain("end");
    expanded.stop();
  });
});

function mountFeed(options: {
  items?: WidgetFeedExchange[];
  hasEarlier?: boolean;
  nextBefore?: string | null;
  token?: string;
  getWidgetFeed?: ViewerApi["getWidgetFeed"];
  getWidgetMessage?: ViewerApi["getWidgetMessage"];
  openWidgetEvent?: ViewerApi["openWidgetEvent"];
  status?: ViewerStatus;
  api?: ViewerApi;
}): { root: HTMLDivElement; stop: () => void } {
  const root = document.createElement("div");
  root.className = "widget-surface-shell";
  document.body.append(root);
  const page = feedPage({
    items: options.items ?? [],
    hasEarlier: options.hasEarlier,
    nextBefore: options.nextBefore,
    token: options.token,
  });
  const api =
    options.api ??
    ({
      getStatus: async () => options.status ?? watchingStatus(),
      getWidgetFeed: options.getWidgetFeed ?? (async () => page),
      getWidgetMessage: options.getWidgetMessage ?? (async () => null),
      openWidgetEvent: options.openWidgetEvent ?? (async () => undefined),
    } as ViewerApi);
  const handle = mountWidgetFeed({
    parent: root,
    api,
    takePointerReassertion: async () => undefined,
    onPresented: () => undefined,
    onPolled: () => undefined,
  });
  return { root, stop: handle.stop };
}

function feedPage(options: {
  items: WidgetFeedExchange[];
  hasEarlier?: boolean;
  nextBefore?: string | null;
  token?: string;
  totalExchanges?: number;
  resetRequired?: boolean;
}): WidgetFeedPage {
  return {
    historyToken: options.token ?? "history-1",
    items: options.items,
    nextBeforeExchangeKey: options.nextBefore ?? null,
    hasEarlier: options.hasEarlier === true,
    totalExchanges: options.totalExchanges ?? options.items.length,
    totalEvents: options.items.length,
    resetRequired: options.resetRequired === true,
  };
}

function range(start: number, end: number): WidgetFeedExchange[] {
  const items: WidgetFeedExchange[] = [];
  for (let index = start; index < end; index += 1) {
    items.push(exchange(index, { requestBody: `Request ${index}`, responseBody: null }));
  }
  return items;
}

function exchange(
  index: number,
  options: {
    sessionKey?: string;
    timestampMs?: number;
    requestBody?: string;
    responseBody?: string | null;
    truncated?: boolean;
    fullCharacterLength?: number;
    contextOmitted?: boolean;
    projection?: WidgetFeedMessage["projection"];
  } = {},
): WidgetFeedExchange {
  const responseBody = options.responseBody === undefined ? `Response ${index}` : options.responseBody;
  return {
    exchangeKey: `exchange-${index}`,
    sessionKey: options.sessionKey ?? "session-a",
    timestampMs: options.timestampMs ?? index * 1_000,
    request: message("request-" + String(index), "request", "codex", "grok", options.requestBody ?? `Request ${index}`, {
      contextOmitted: options.contextOmitted,
      projection: options.projection,
    }),
    completion:
      responseBody == null
        ? null
        : message("response-" + String(index), "response", "grok", "codex", responseBody, {
            truncated: options.truncated,
            fullCharacterLength: options.fullCharacterLength,
          }),
    pendingLabel: null,
  };
}

function pendingExchange(index: number, label: string): WidgetFeedExchange {
  return {
    exchangeKey: `exchange-${index}`,
    sessionKey: "session-a",
    timestampMs: index * 1_000,
    request: message(`request-${index}`, "request", "codex", "grok", `Request ${index}`),
    completion: null,
    pendingLabel: label,
  };
}

function message(
  eventKey: string,
  eventType: WidgetFeedMessage["eventType"],
  speaker: string,
  recipient: string,
  body: string,
  options: {
    truncated?: boolean;
    fullCharacterLength?: number;
    contextOmitted?: boolean;
    projection?: WidgetFeedMessage["projection"];
  } = {},
): WidgetFeedMessage {
  return {
    eventKey,
    eventType,
    speaker,
    recipient,
    timestampMs: 1_700_000_000_000,
    status: eventType === "error" ? "failed" : "ok",
    body,
    fullCharacterLength: options.fullCharacterLength ?? Array.from(body).length,
    truncated: options.truncated === true,
    projection: options.projection ?? "exact",
    contextOmitted: options.contextOmitted === true,
  };
}

function watchingStatus(): ViewerStatus {
  return {
    sourceState: "watching",
    generation: 1,
    bytesRead: 100,
    sessionCount: 1,
    exchangeCount: 1,
    lastEventTimestampMs: 0,
    trayAvailable: true,
    underlayState: "attached",
    desktopRuntimeState: "interactive",
    desktopFallbackReason: null,
    widgetVisible: true,
    diagnostics: {
      malformedLines: 0,
      oversizedLines: 0,
      unsupportedRecords: 0,
      duplicateEvents: 0,
      ioErrors: 0,
      aliasCollisions: 0,
      lastError: null,
    },
    sources: [],
  };
}

function scrollport(root: ParentNode): HTMLElement {
  const node = root.querySelector<HTMLElement>(`#${LANDMARKS.widgetFeedScroll}`);
  if (!node) {
    throw new Error("Missing scrollport");
  }
  return node;
}

function button(root: ParentNode, id: string): HTMLButtonElement {
  const node = root.querySelector<HTMLButtonElement>(`#${id}`);
  if (!node) {
    throw new Error(`Missing ${id}`);
  }
  return node;
}

function row(root: ParentNode, eventKey: string): HTMLElement {
  const node = root.querySelector<HTMLElement>(`[data-event-key="${eventKey}"]`);
  if (!node) {
    throw new Error(`Missing ${eventKey}`);
  }
  return node;
}

async function bodyFor(root: ParentNode, eventKey: string): Promise<HTMLElement> {
  await vi.waitFor(() => {
    expect(root.querySelector(`[data-event-key="${eventKey}"] .widget-feed-body`)).not.toBeNull();
  });
  const body = root.querySelector<HTMLElement>(`[data-event-key="${eventKey}"] .widget-feed-body`);
  if (!body) {
    throw new Error(`Missing body ${eventKey}`);
  }
  return body;
}

function exchangeKeys(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>("[data-exchange-key]")].map(
    (node) => node.dataset.exchangeKey ?? "",
  );
}

function stubMetrics(node: HTMLElement, metrics: { scrollHeight: number; clientHeight: number }): void {
  let top = node.scrollTop;
  Object.defineProperty(node, "scrollHeight", { configurable: true, get: () => metrics.scrollHeight });
  Object.defineProperty(node, "clientHeight", { configurable: true, get: () => metrics.clientHeight });
  Object.defineProperty(node, "scrollTop", {
    configurable: true,
    get: () => top,
    set: (value: number) => {
      top = value;
      node.dispatchEvent(new Event("scroll"));
    },
  });
}

function distance(node: HTMLElement): number {
  return node.scrollHeight - node.scrollTop - node.clientHeight;
}
