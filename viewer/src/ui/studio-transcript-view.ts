import { el, setText } from "./dom";
import { LANDMARKS } from "./landmarks";
import type {
  StudioTranscriptCallbacks,
  StudioTranscriptNodes,
  TranscriptPaint,
  TranscriptRowModel,
} from "./studio-seam";
import { isFocusLocked, isTimelineKey, moveSearchStop } from "./timeline";

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

export function createStudioTranscript(callbacks: StudioTranscriptCallbacks): StudioTranscriptNodes {
  const searchInput = el("input", {
    attrs: {
      type: "search",
      name: "query",
      placeholder: "Search conversation",
      "aria-label": "Search conversation",
      autocomplete: "off",
      spellcheck: "false",
    },
  });
  const searchForm = el("form", {
    className: "studio-focus-search",
    attrs: { "data-region": LANDMARKS.studioSearch, role: "search" },
    children: [
      searchInput,
      el("button", { className: "studio-focus-search-submit", text: "Search", attrs: { type: "submit" } }),
    ],
  });
  const meta = el("p", { className: "studio-focus-meta" });
  const jumpLatest = el("button", {
    className: "studio-focus-jump",
    text: "Jump to latest",
    attrs: { type: "button" },
  });
  jumpLatest.hidden = true;
  const empty = el("p", { className: "studio-focus-empty" });
  empty.hidden = true;
  const list = el("div", { className: "studio-focus-list" });
  const loadEarlier = el("button", {
    className: "studio-focus-earlier",
    text: "Load earlier messages",
    attrs: { type: "button" },
  });
  loadEarlier.hidden = true;
  const scrollport = el("div", {
    className: "studio-focus-scroll",
    attrs: { "data-region": LANDMARKS.studioTranscriptScroll },
    children: [empty, list, loadEarlier],
  });
  const region = el("section", {
    className: "studio-focus-transcript",
    attrs: { "data-region": LANDMARKS.studioTranscript, "aria-label": "Conversation" },
    children: [searchForm, meta, jumpLatest, scrollport],
  });

  searchForm.addEventListener("submit", (event) => {
    event.preventDefault();
    callbacks.onSearch(searchInput.value);
  });
  jumpLatest.addEventListener("click", () => {
    callbacks.onJumpLatest();
  });
  loadEarlier.addEventListener("click", () => {
    callbacks.onLoadEarlier();
  });
  list.addEventListener("keydown", (event) => {
    if (isFocusLocked(event.target)) {
      return;
    }
    const buttons = messageButtons(list);
    if (buttons.length === 0) {
      return;
    }
    const current = buttons.findIndex((button) => button === document.activeElement);
    const active = current >= 0 ? buttons[current] : undefined;
    if (event.key === "Enter" || event.key === " ") {
      if (!active?.dataset.eventKey || !active.dataset.exchangeKey) {
        return;
      }
      event.preventDefault();
      active.click();
      return;
    }
    if (!isTimelineKey(event.key)) {
      return;
    }
    event.preventDefault();
    const nextIndex = moveSearchStop(buttons.length, current, event.key);
    const target = buttons[nextIndex];
    const eventKey = target?.dataset.eventKey;
    if (!target || !eventKey) {
      return;
    }
    for (const button of buttons) {
      button.tabIndex = button === target ? 0 : -1;
    }
    target.focus();
    callbacks.onFocus(eventKey);
  });

  return {
    region,
    scrollport,
    list,
    searchForm,
    searchInput,
    loadEarlier,
    jumpLatest,
    meta,
    empty,
    paint: (input) => paintTranscript(list, meta, empty, jumpLatest, loadEarlier, region, callbacks, input),
  };
}

function paintTranscript(
  list: HTMLDivElement,
  meta: HTMLParagraphElement,
  empty: HTMLParagraphElement,
  jumpLatest: HTMLButtonElement,
  loadEarlier: HTMLButtonElement,
  region: HTMLElement,
  callbacks: StudioTranscriptCallbacks,
  input: TranscriptPaint,
): void {
  region.dataset.transcriptMode = input.mode;
  setText(meta, input.meta);
  setText(jumpLatest, input.jumpLatestLabel);
  jumpLatest.hidden = !input.showJumpLatest;
  loadEarlier.hidden = !input.hasEarlier;
  if (input.emptyMessage === null) {
    empty.hidden = true;
    setText(empty, "");
  } else {
    empty.hidden = false;
    setText(empty, input.emptyMessage);
  }

  list.replaceChildren();
  let previousDate = "";
  for (const row of input.rows) {
    const date = dateLabel(row);
    if (date !== previousDate) {
      list.append(el("p", { className: "studio-focus-date", text: date }));
      previousDate = date;
    }
    list.append(renderRow(row, input, callbacks));
  }

  const buttons = messageButtons(list);
  const preferred =
    buttons.find((button) => button.dataset.eventKey === input.focusedEventKey) ?? buttons[0];
  for (const button of buttons) {
    button.tabIndex = button === preferred ? 0 : -1;
  }
}

function renderRow(
  row: TranscriptRowModel,
  input: TranscriptPaint,
  callbacks: StudioTranscriptCallbacks,
): HTMLElement {
  const speaker = speakerLabel(row.speaker);
  const body = [
    el("span", { className: "studio-focus-speaker", text: speaker }),
    el("span", { className: "studio-focus-role", text: row.roleLabel }),
    el("time", {
      className: "studio-focus-time",
      text: row.timeLabel,
      attrs: { datetime: dateTime(row.timestampMs) },
    }),
    el("p", { className: "studio-focus-excerpt", text: row.excerpt }),
  ];
  if (row.pending) {
    return el("div", {
      className: "studio-focus-row is-pending",
      attrs: row.speaker ? { "data-pending": "true", "data-speaker": row.speaker } : { "data-pending": "true" },
      children: body,
    });
  }

  const selected = row.eventKey === input.selectedEventKey;
  const control = el("button", {
    className: selected ? "studio-focus-row is-selected" : "studio-focus-row",
    attrs: {
      type: "button",
      "data-event-key": row.eventKey,
      "data-exchange-key": row.exchangeKey,
      "data-kind": row.kind,
      ...(row.speaker ? { "data-speaker": row.speaker } : {}),
      ...(selected ? { "aria-current": "true" } : {}),
    },
    children: body,
  });
  control.addEventListener("click", () => {
    callbacks.onActivate(row.eventKey, row.exchangeKey);
  });
  return control;
}

function messageButtons(list: ParentNode): HTMLButtonElement[] {
  return Array.from(list.querySelectorAll<HTMLButtonElement>("button[data-event-key]"));
}

function speakerLabel(speaker: string | null): string {
  if (speaker === "codex") {
    return "Codex";
  }
  if (speaker === "grok") {
    return "Grok";
  }
  if (speaker === null || speaker.trim() === "") {
    return "Unknown speaker";
  }
  return speaker;
}

function dateLabel(row: TranscriptRowModel): string {
  const marker = " at ";
  const index = row.timeLabel.lastIndexOf(marker);
  if (index > 0) {
    return row.timeLabel.slice(0, index);
  }
  if (!Number.isFinite(row.timestampMs)) {
    return "Unknown date";
  }
  const date = new Date(row.timestampMs);
  if (Number.isNaN(date.getTime())) {
    return "Unknown date";
  }
  const month = MONTHS[date.getUTCMonth()];
  return month ? `${date.getUTCDate()} ${month} ${date.getUTCFullYear()}` : "Unknown date";
}

function dateTime(timestampMs: number): string | undefined {
  if (!Number.isFinite(timestampMs)) {
    return undefined;
  }
  const date = new Date(timestampMs);
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}
