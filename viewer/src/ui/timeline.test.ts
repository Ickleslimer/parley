/**
 * @vitest-environment happy-dom
 */
import { describe, expect, it } from "vitest";

import type { ExchangeSummary, MessagePreview, SearchHit, SessionSummary } from "../contracts";

import {
  exchangeStops,
  moveSearchStop,
  moveTimelineStop,
  paintExchangeTimeline,
  paintSearchTimeline,
  paintSessionRail,
} from "./timeline";

const HOSTILE = "<img src=x onerror=alert(1)>";

function preview(overrides: Partial<MessagePreview> = {}): MessagePreview {
  return {
    eventKey: "event:request",
    eventId: "request-id",
    eventType: "request",
    speaker: "codex",
    recipient: "grok",
    timestampMs: 1_700_000_000_000,
    status: "ok",
    excerpt: HOSTILE,
    excerptExtracted: false,
    contentLength: HOSTILE.length,
    ...overrides,
  };
}

function exchange(overrides: Partial<ExchangeSummary> = {}): ExchangeSummary {
  return {
    exchangeKey: "exchange:1",
    sessionKey: "session:1",
    exchangeId: "exchange-1",
    sessionId: "session-1",
    sourcePath: "synthetic://events.jsonl",
    timestampMs: 1_700_000_000_000,
    request: preview(),
    completion: preview({
      eventKey: "event:completion",
      eventId: "completion-id",
      eventType: "response",
      speaker: "grok",
      recipient: "codex",
      excerpt: "Exact completion",
    }),
    pendingLabel: null,
    ...overrides,
  };
}

describe("timeline movement", () => {
  const stops = exchangeStops([
    exchange(),
    exchange({
      exchangeKey: "exchange:2",
      exchangeId: "exchange-2",
      request: preview({ eventKey: "event:request-2", speaker: "codex" }),
      completion: null,
      pendingLabel: "Request logged; no response event yet",
    }),
    exchange({
      exchangeKey: "exchange:3",
      exchangeId: "exchange-3",
      request: preview({ eventKey: "event:request-3" }),
      completion: preview({ eventKey: "event:completion-3", eventType: "response", speaker: "grok" }),
    }),
  ]);

  it("moves within an exchange and keeps the same side when the next exchange has it", () => {
    expect(moveTimelineStop(stops, 0, "ArrowRight")).toBe(1);
    expect(moveTimelineStop(stops, 1, "ArrowRight")).toBe(1);
    expect(moveTimelineStop(stops, 1, "ArrowLeft")).toBe(0);
    expect(moveTimelineStop(stops, 0, "ArrowDown")).toBe(2);
    expect(stops[2]?.side).toBe("request");
    expect(moveTimelineStop(stops, 1, "ArrowDown")).toBe(2);
    expect(moveTimelineStop(stops, 2, "ArrowDown")).toBe(3);
    expect(stops[3]?.side).toBe("request");
    expect(moveTimelineStop(stops, 4, "ArrowUp")).toBe(2);
  });

  it("uses home and end for the rendered page and leaves search rows flat", () => {
    expect(moveTimelineStop(stops, 2, "Home")).toBe(0);
    expect(moveTimelineStop(stops, 2, "End")).toBe(stops.length - 1);
    expect(moveSearchStop(3, 1, "ArrowDown")).toBe(2);
    expect(moveSearchStop(3, 1, "ArrowLeft")).toBe(1);
    expect(moveSearchStop(3, 1, "Home")).toBe(0);
    expect(moveSearchStop(3, 0, "End")).toBe(2);
  });
});

describe("timeline rendering", () => {
  it("escapes hostile excerpts and roves focus without selecting", () => {
    const list = document.createElement("div");
    document.body.append(list);
    const chosen: string[] = [];
    const second = exchange({
      exchangeKey: "exchange:2",
      request: preview({ eventKey: "event:request-2", excerpt: "Second request" }),
      completion: preview({
        eventKey: "event:completion-2",
        eventType: "response",
        speaker: "grok",
        excerpt: "Second completion",
      }),
    });
    paintExchangeTimeline(list, [exchange(), second], null, (_exchange, eventKey) => {
      chosen.push(eventKey);
    });

    expect(list.querySelector("img")).toBeNull();
    expect(list.textContent).toContain(HOSTILE);
    const buttons = Array.from(list.querySelectorAll<HTMLButtonElement>(".studio-message"));
    expect(buttons.map((button) => button.tabIndex)).toEqual([0, -1, -1, -1]);
    buttons[0]?.focus();
    buttons[0]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(document.activeElement).toBe(buttons[1]);
    expect(buttons[1]?.getAttribute("aria-pressed")).toBe("false");
    expect(chosen).toEqual([]);
    buttons[1]?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(chosen).toEqual(["event:completion"]);
    buttons[1]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:completion-2");
  });

  it("keeps a focused message focused when the same keys are painted again", () => {
    const list = document.createElement("div");
    document.body.append(list);
    const rows = [exchange()];
    paintExchangeTimeline(list, rows, "event:request", () => undefined);
    const completion = list.querySelector<HTMLButtonElement>('[data-event-key="event:completion"]');
    completion?.focus();
    paintExchangeTimeline(list, rows, "event:request", () => undefined);
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:completion");
    expect(list.querySelector('[data-event-key="event:request"]')?.getAttribute("aria-pressed")).toBe(
      "true",
    );
  });

  it("does not move focus out of the search field while repainting", () => {
    const list = document.createElement("div");
    const input = document.createElement("input");
    document.body.append(input, list);
    input.focus();
    paintExchangeTimeline(list, [exchange()], null, () => undefined);
    expect(document.activeElement).toBe(input);
  });

  it("moves session and search options with the keyboard and preserves exact hit text", () => {
    const sessions = document.createElement("ul");
    const results = document.createElement("div");
    document.body.append(sessions, results);
    const sessionRows: SessionSummary[] = [
      {
        sessionKey: "session:1",
        sessionId: "one",
        sourcePath: "synthetic://one",
        exchangeCount: 1,
        latestTimestampMs: 1,
        latestSource: "codex",
        latestTarget: "grok",
        latestExcerpt: HOSTILE,
        excerptExtracted: false,
      },
      {
        sessionKey: "session:2",
        sessionId: "two",
        sourcePath: "synthetic://two",
        exchangeCount: 1,
        latestTimestampMs: 2,
        latestSource: "grok",
        latestTarget: "codex",
        latestExcerpt: "Second session",
        excerptExtracted: false,
      },
    ];
    const selected: string[] = [];
    paintSessionRail(sessions, sessionRows, null, (key) => selected.push(key));
    const options = Array.from(sessions.querySelectorAll<HTMLElement>('[role="option"]'));
    options[0]?.focus();
    options[0]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement?.getAttribute("data-session-key")).toBe("session:2");
    expect(options[0]?.getAttribute("aria-selected")).toBe("false");
    document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(selected).toEqual(["session:2"]);
    expect(sessions.querySelector("img")).toBeNull();

    const hits: SearchHit[] = [
      {
        eventKey: "event:hit-1",
        exchangeKey: "exchange:1",
        sessionKey: "session:1",
        eventId: "hit-1",
        exchangeId: "exchange-1",
        sessionId: "one",
        sourcePath: "synthetic://one",
        eventType: "request",
        timestampMs: 1,
        excerpt: HOSTILE,
        matchOffset: 4,
      },
      {
        eventKey: "event:hit-2",
        exchangeKey: "exchange:2",
        sessionKey: "session:1",
        eventId: "hit-2",
        exchangeId: "exchange-2",
        sessionId: "one",
        sourcePath: "synthetic://one",
        eventType: "response",
        timestampMs: 2,
        excerpt: "Second hit",
        matchOffset: 9,
      },
    ];
    const chosen: string[] = [];
    paintSearchTimeline(results, hits, null, (hit) => chosen.push(hit.eventKey));
    const hitOptions = Array.from(results.querySelectorAll<HTMLElement>('[role="option"]'));
    hitOptions[0]?.focus();
    hitOptions[0]?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    expect(document.activeElement?.getAttribute("data-event-key")).toBe("event:hit-2");
    document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
    expect(chosen).toEqual(["event:hit-2"]);
    expect(results.textContent).toContain(HOSTILE);
    expect(results.querySelector("img")).toBeNull();
  });
});
