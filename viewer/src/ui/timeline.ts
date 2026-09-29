import type { ExchangeSummary, SearchHit } from "../contracts";

import { messageBubble, pendingBubble } from "./attribution";
import { el } from "./dom";
import { presentExchange, searchHitHeading, type PresentedMessage } from "./excerpt";
import { formatTimestamp } from "./format";
import { LANDMARKS } from "./landmarks";

export type TimelineSide = "request" | "completion";

export interface TimelineStop {
  exchangeKey: string;
  eventKey: string;
  side: TimelineSide;
}

export type TimelineKey =
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "ArrowDown"
  | "Home"
  | "End";

const TIMELINE_KEYS = new Set<string>([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
]);

export function isTimelineKey(key: string): key is TimelineKey {
  return TIMELINE_KEYS.has(key);
}

export function isFocusLocked(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest("input, select, textarea, pre") != null;
}

export function exchangeStops(exchanges: readonly ExchangeSummary[]): TimelineStop[] {
  const stops: TimelineStop[] = [];
  for (const exchange of exchanges) {
    const presented = presentExchange(exchange);
    if (presented.request) {
      stops.push({
        exchangeKey: exchange.exchangeKey,
        eventKey: presented.request.eventKey,
        side: "request",
      });
    }
    if (presented.completion) {
      stops.push({
        exchangeKey: exchange.exchangeKey,
        eventKey: presented.completion.eventKey,
        side: "completion",
      });
    }
  }
  return stops;
}

export function moveTimelineStop(
  stops: readonly TimelineStop[],
  currentIndex: number,
  key: TimelineKey,
): number {
  if (stops.length === 0) {
    return -1;
  }
  if (currentIndex < 0 || currentIndex >= stops.length) {
    return key === "End" ? stops.length - 1 : 0;
  }
  if (key === "Home") {
    return 0;
  }
  if (key === "End") {
    return stops.length - 1;
  }
  const current = stops[currentIndex];
  if (!current) {
    return 0;
  }
  if (key === "ArrowLeft" || key === "ArrowRight") {
    const same: number[] = [];
    stops.forEach((stop, index) => {
      if (stop.exchangeKey === current.exchangeKey) {
        same.push(index);
      }
    });
    const local = same.indexOf(currentIndex);
    const next = same[key === "ArrowLeft" ? local - 1 : local + 1];
    return next === undefined ? currentIndex : next;
  }
  const exchangeKeys: string[] = [];
  for (const stop of stops) {
    if (exchangeKeys[exchangeKeys.length - 1] !== stop.exchangeKey) {
      exchangeKeys.push(stop.exchangeKey);
    }
  }
  const exchangeIndex = exchangeKeys.indexOf(current.exchangeKey);
  const nextKey = exchangeKeys[exchangeIndex + (key === "ArrowDown" ? 1 : -1)];
  if (!nextKey) {
    return currentIndex;
  }
  const candidates: number[] = [];
  stops.forEach((stop, index) => {
    if (stop.exchangeKey === nextKey) {
      candidates.push(index);
    }
  });
  const corresponding = candidates.find((index) => stops[index]?.side === current.side);
  return corresponding ?? candidates[0] ?? currentIndex;
}

export function moveSearchStop(count: number, currentIndex: number, key: TimelineKey): number {
  if (count <= 0) {
    return -1;
  }
  if (currentIndex < 0 || currentIndex >= count) {
    return key === "End" ? count - 1 : 0;
  }
  if (key === "Home") {
    return 0;
  }
  if (key === "End") {
    return count - 1;
  }
  if (key === "ArrowDown") {
    return Math.min(count - 1, currentIndex + 1);
  }
  if (key === "ArrowUp") {
    return Math.max(0, currentIndex - 1);
  }
  return currentIndex;
}

export interface TimelineNodes {
  region: HTMLElement;
  title: HTMLHeadingElement;
  form: HTMLFormElement;
  input: HTMLInputElement;
  meta: HTMLParagraphElement;
  previous: HTMLButtonElement;
  next: HTMLButtonElement;
  empty: HTMLParagraphElement;
  list: HTMLDivElement;
}

export function buildTimeline(): TimelineNodes {
  const title = el("h2", { className: "studio-timeline-title", text: "Conversation" });
  const input = el("input", {
    attrs: {
      type: "search",
      name: "query",
      placeholder: "Search exact content",
      "aria-label": "Search exact event content",
      autocomplete: "off",
      spellcheck: "false",
    },
  });
  const submit = el("button", {
    className: "studio-action",
    text: "Search",
    attrs: { type: "submit" },
  });
  const form = el("form", {
    className: "studio-search",
    attrs: { "data-region": LANDMARKS.studioSearch, role: "search" },
    children: [input, submit],
  });
  const meta = el("p", { className: "studio-pager-meta" });
  const previous = pagerButton("Previous");
  const next = pagerButton("Next");
  const empty = el("p", { className: "studio-empty" });
  const list = el("div", {
    className: "studio-thread",
    attrs: { role: "list", "aria-label": "Exchanges" },
  });
  const region = el("section", {
    className: "studio-timeline",
    attrs: { "data-region": LANDMARKS.studioTimeline, "aria-label": "Conversation" },
    children: [
      title,
      form,
      el("div", { className: "studio-pager", children: [previous, meta, next] }),
      empty,
      list,
    ],
  });
  return { region, title, form, input, meta, previous, next, empty, list };
}

export function paintExchangeTimeline(
  list: HTMLDivElement,
  exchanges: readonly ExchangeSummary[],
  selectedEventKey: string | null,
  onSelect: (exchange: ExchangeSummary, eventKey: string) => void,
): void {
  const restoreKey = focusedAttribute(list, "[data-event-key]", "data-event-key");
  const preferred = preferredKey(
    exchangeStops(exchanges).map((stop) => stop.eventKey),
    selectedEventKey,
    restoreKey,
  );
  list.setAttribute("role", "list");
  list.setAttribute("aria-label", "Exchanges");
  list.replaceChildren();
  const exchangesByKey = new Map(exchanges.map((exchange) => [exchange.exchangeKey, exchange]));
  for (const exchange of exchanges) {
    list.append(renderExchange(exchange, selectedEventKey, preferred, onSelect));
  }
  bindExchangeKeys(list, (eventKey, exchangeKey) => {
    const exchange = exchangesByKey.get(exchangeKey);
    if (exchange) {
      onSelect(exchange, eventKey);
    }
  });
  restoreAttribute(list, "[data-event-key]", "data-event-key", restoreKey);
}

export function paintSearchTimeline(
  list: HTMLDivElement,
  hits: readonly SearchHit[],
  selectedEventKey: string | null,
  onSelect: (hit: SearchHit) => void,
): void {
  const restoreKey = focusedAttribute(list, "[data-event-key]", "data-event-key");
  const preferred = preferredKey(
    hits.map((hit) => hit.eventKey),
    selectedEventKey,
    restoreKey,
  );
  list.setAttribute("role", "listbox");
  list.setAttribute("aria-label", "Search results");
  list.replaceChildren();
  const hitsByKey = new Map(hits.map((hit) => [hit.eventKey, hit]));
  for (const hit of hits) {
    const selected = hit.eventKey === selectedEventKey;
    const option = el("div", {
      className: selected ? "studio-search-hit is-selected" : "studio-search-hit",
      attrs: {
        role: "option",
        tabindex: hit.eventKey === preferred ? "0" : "-1",
        "aria-selected": selected ? "true" : "false",
        "data-event-key": hit.eventKey,
        "data-exchange-key": hit.exchangeKey,
      },
    });
    option.append(
      el("span", {
        className: "studio-message-heading",
        text: `${searchHitHeading(hit.eventType)}, ${formatTimestamp(hit.timestampMs)}`,
      }),
      el("span", { className: "studio-excerpt", text: hit.excerpt }),
    );
    option.addEventListener("click", () => {
      option.focus();
      onSelect(hit);
    });
    list.append(option);
  }
  bindOptionKeys(list, (option) => {
    const key = option.getAttribute("data-event-key");
    const hit = key ? hitsByKey.get(key) : undefined;
    if (hit) {
      onSelect(hit);
    }
  });
  restoreAttribute(list, "[data-event-key]", "data-event-key", restoreKey);
}

function renderExchange(
  exchange: ExchangeSummary,
  selectedEventKey: string | null,
  preferred: string | null,
  onSelect: (exchange: ExchangeSummary, eventKey: string) => void,
): HTMLElement {
  const presented = presentExchange(exchange);
  const article = el("article", {
    className: "studio-exchange",
    attrs: { "data-exchange-key": exchange.exchangeKey },
  });
  article.append(
    el("p", {
      className: "studio-exchange-meta",
      text: formatTimestamp(exchange.timestampMs),
    }),
  );
  if (presented.request) {
    article.append(
      messageControl(presented.request, exchange, "request", selectedEventKey, preferred, onSelect),
    );
  }
  if (presented.completion) {
    article.append(
      messageControl(
        presented.completion,
        exchange,
        "completion",
        selectedEventKey,
        preferred,
        onSelect,
      ),
    );
  } else if (presented.pendingLabel) {
    const recipient = presented.request?.recipient ?? "";
    const pending = pendingBubble(presented.pendingLabel, recipient);
    article.append(
      el("p", {
        className: `studio-pending is-${pending.speaker} tail-${pending.tail}`,
        children: [
          el("span", { className: "studio-message-name", text: pending.displayName }),
          el("span", { className: "studio-excerpt", text: pending.excerpt }),
        ],
      }),
    );
  }
  return article;
}

function messageControl(
  message: PresentedMessage,
  exchange: ExchangeSummary,
  side: TimelineSide,
  selectedEventKey: string | null,
  preferred: string | null,
  onSelect: (exchange: ExchangeSummary, eventKey: string) => void,
): HTMLButtonElement {
  const bubble = messageBubble(message);
  const selected = message.eventKey === selectedEventKey;
  const control = el("button", {
    className: `studio-message is-${bubble.kind} is-${bubble.speaker} tail-${bubble.tail}`,
    attrs: {
      type: "button",
      tabindex: message.eventKey === preferred ? "0" : "-1",
      "aria-pressed": selected ? "true" : "false",
      "data-event-key": message.eventKey,
      "data-exchange-key": exchange.exchangeKey,
      "data-side": side,
    },
    children: [
      el("span", { className: "studio-message-name", text: bubble.displayName }),
      el("span", { className: "studio-message-heading", text: message.heading }),
      el("span", { className: "studio-message-time", text: message.timestamp }),
      message.extractedLabel
        ? el("span", { className: "studio-flag", text: message.extractedLabel })
        : null,
      el("span", { className: "studio-excerpt", text: message.excerpt }),
    ],
  });
  control.addEventListener("click", () => {
    control.focus();
    onSelect(exchange, message.eventKey);
  });
  return control;
}

function bindExchangeKeys(
  list: HTMLElement,
  onSelect: (eventKey: string, exchangeKey: string) => void,
): void {
  list.onkeydown = (event: KeyboardEvent) => {
    if (isFocusLocked(event.target)) {
      return;
    }
    const buttons = messageButtons(list);
    if (buttons.length === 0) {
      return;
    }
    const current = buttons.findIndex((button) => button === document.activeElement);
    if (event.key === "Enter" || event.key === " ") {
      const button = current >= 0 ? buttons[current] : null;
      const eventKey = button?.dataset.eventKey;
      const exchangeKey = button?.dataset.exchangeKey;
      if (!button || !eventKey || !exchangeKey) {
        return;
      }
      event.preventDefault();
      onSelect(eventKey, exchangeKey);
      return;
    }
    if (!isTimelineKey(event.key)) {
      return;
    }
    event.preventDefault();
    const stops: TimelineStop[] = buttons.map((button) => ({
      exchangeKey: button.dataset.exchangeKey ?? "",
      eventKey: button.dataset.eventKey ?? "",
      side: button.dataset.side === "completion" ? "completion" : "request",
    }));
    focusRoving(buttons, moveTimelineStop(stops, current, event.key));
  };
}

function bindOptionKeys(list: HTMLElement, onActivate: (option: HTMLElement) => void): void {
  list.onkeydown = (event: KeyboardEvent) => {
    if (isFocusLocked(event.target)) {
      return;
    }
    const options = Array.from(list.querySelectorAll<HTMLElement>('[role="option"]'));
    if (options.length === 0) {
      return;
    }
    const current = options.findIndex(
      (option) => option === document.activeElement || option.contains(document.activeElement),
    );
    if (event.key === "Enter" || event.key === " ") {
      const option = current >= 0 ? options[current] : null;
      if (!option) {
        return;
      }
      event.preventDefault();
      onActivate(option);
      return;
    }
    if (!isTimelineKey(event.key)) {
      return;
    }
    event.preventDefault();
    focusRoving(options, moveSearchStop(options.length, current, event.key));
  };
}

function messageButtons(list: ParentNode): HTMLButtonElement[] {
  return Array.from(list.querySelectorAll<HTMLButtonElement>(".studio-message"));
}

function focusRoving(items: readonly HTMLElement[], index: number): void {
  const target = items[index];
  if (!target) {
    return;
  }
  for (const item of items) {
    item.tabIndex = -1;
  }
  target.tabIndex = 0;
  target.focus();
}

function preferredKey(
  keys: readonly string[],
  selected: string | null,
  restore: string | null,
): string | null {
  if (restore && keys.includes(restore)) {
    return restore;
  }
  if (selected && keys.includes(selected)) {
    return selected;
  }
  return keys[0] ?? null;
}

function focusedAttribute(container: ParentNode, selector: string, attribute: string): string | null {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement) || isFocusLocked(active) || !container.contains(active)) {
    return null;
  }
  const owner = active.closest<HTMLElement>(selector);
  if (!owner || !container.contains(owner)) {
    return null;
  }
  return owner.getAttribute(attribute);
}

function restoreAttribute(
  container: ParentNode,
  selector: string,
  attribute: string,
  key: string | null,
): void {
  if (!key || isFocusLocked(document.activeElement)) {
    return;
  }
  const match = Array.from(container.querySelectorAll<HTMLElement>(selector)).find(
    (node) => node.getAttribute(attribute) === key,
  );
  match?.focus();
}

function pagerButton(label: string): HTMLButtonElement {
  return el("button", {
    className: "studio-action",
    text: label,
    attrs: { type: "button" },
  });
}
