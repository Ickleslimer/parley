import type { BubbleModel, WidgetSceneModel } from "./attribution";
import { el, setText } from "./dom";
import { LANDMARKS } from "./landmarks";

export interface PaperColumnOptions {
  liveId: string;
  columnId?: string;
  liveOwner?: boolean;
  figures?: { codex: HTMLImageElement; grok: HTMLImageElement };
}

export interface PaperColumnHandle {
  column: HTMLDivElement;
  live: HTMLDivElement;
  paint(model: WidgetSceneModel): void;
  setLiveOwner(owner: boolean): void;
}

export interface WidgetUnderlayHandle {
  column: HTMLDivElement;
}

export function createPaperColumn(options: PaperColumnOptions): PaperColumnHandle {
  const banner = el("p", { className: "widget-banner" });
  banner.hidden = true;
  const loadError = el("p", { className: "widget-load-error" });
  loadError.hidden = true;
  const idle = el("p", { className: "widget-idle" });
  const messages = el("div", { className: "widget-messages" });
  const live = el("div", {
    id: options.liveId,
    className: "widget-live",
    children: [banner, loadError, idle, messages],
  });
  const source = el("p", { className: "widget-source" });
  const column = el("div", {
    className: "widget-column",
    ...(options.columnId ? { id: options.columnId } : {}),
    children: [live, source],
  });
  let paintedRevision: string | null = null;

  const setLiveOwner = (owner: boolean): void => {
    if (owner) {
      live.setAttribute("role", "status");
      live.setAttribute("aria-live", "polite");
      live.setAttribute("aria-atomic", "true");
      return;
    }
    live.setAttribute("aria-live", "off");
    live.removeAttribute("role");
    live.removeAttribute("aria-atomic");
  };

  setLiveOwner(options.liveOwner !== false);

  return {
    column,
    live,
    setLiveOwner,
    paint(model) {
      setText(source, model.sourceLabel);
      const revision = visibleRevision(model);
      if (revision === paintedRevision) {
        return;
      }
      paintedRevision = revision;
      paintNotice(banner, model.banner);
      paintNotice(loadError, model.loadError);
      const showIdle = Boolean(model.idleLabel) && model.bubbles.length === 0 && !model.loadError;
      idle.hidden = !showIdle;
      setText(idle, showIdle && model.idleLabel ? model.idleLabel : "");
      syncBubbles(messages, model.bubbles, options.figures);
    },
  };
}

export function createWidgetUnderlayScene(root: HTMLElement): WidgetUnderlayHandle {
  root.className = "widget-shell";
  root.removeAttribute("role");
  root.removeAttribute("aria-live");
  root.removeAttribute("aria-atomic");
  root.removeAttribute("tabindex");

  const column = el("div", {
    id: LANDMARKS.widgetColumn,
    className: "widget-underlay-host",
    attrs: {
      "aria-hidden": "true",
    },
  });
  const scene = el("section", {
    id: LANDMARKS.widgetScene,
    className: "widget-scene widget-underlay-scene",
    children: [column],
  });
  root.replaceChildren(scene);

  return { column };
}

function visibleRevision(model: WidgetSceneModel): string {
  return [
    model.liveRevision,
    model.bubbles.map((bubble) => `${bubble.displayName}\u001f${bubble.tail}`).join("\u001e"),
    model.banner ?? "",
    model.idleLabel ?? "",
    model.loadError ?? "",
  ].join("\u001d");
}

function paintNotice(node: HTMLElement, text: string | null): void {
  if (text) {
    node.hidden = false;
    setText(node, text);
    return;
  }
  node.hidden = true;
  setText(node, "");
}

function syncBubbles(
  container: HTMLElement,
  bubbles: BubbleModel[],
  figures?: { codex: HTMLImageElement; grok: HTMLImageElement },
): void {
  container.dataset.bubbleCount = String(bubbles.length);
  const previous = new Map<string, HTMLElement>();
  for (const child of Array.from(container.children)) {
    if (child instanceof HTMLElement && child.dataset.bubbleKey) {
      previous.set(child.dataset.bubbleKey, child);
    }
  }
  const ordered: HTMLElement[] = [];
  let reactCodex = false;
  let reactGrok = false;
  for (const bubble of bubbles) {
    const key = bubbleKey(bubble);
    const existing = previous.get(key);
    if (existing) {
      fillSlip(existing, bubble);
      ordered.push(existing);
      continue;
    }
    const created = createSlip(bubble);
    created.dataset.bubbleKey = key;
    if (decorativeMotion()) {
      created.classList.add("is-reacting");
      if (bubble.tail === "toward-codex") {
        reactCodex = true;
      } else if (bubble.tail === "toward-grok") {
        reactGrok = true;
      }
    }
    ordered.push(created);
  }
  const unchanged =
    container.childElementCount === ordered.length &&
    ordered.every((node, index) => container.children.item(index) === node);
  if (!unchanged) {
    container.replaceChildren(...ordered);
  }
  if (reactCodex && figures) {
    poke(figures.codex);
  }
  if (reactGrok && figures) {
    poke(figures.grok);
  }
}

function bubbleKey(bubble: BubbleModel): string {
  if (bubble.eventKey) {
    return `${bubble.kind}:${bubble.eventKey}`;
  }
  return `${bubble.kind}:${bubble.speaker}:${bubble.heading}:${bubble.excerpt}`;
}

function createSlip(bubble: BubbleModel): HTMLDivElement {
  const slip = el("div", {
    className: "widget-slip",
    children: [
      el("p", { className: "widget-name" }),
      el("p", { className: "widget-heading" }),
      el("p", { className: "widget-time" }),
      el("p", { className: "widget-extracted" }),
      el("p", { className: "widget-excerpt" }),
    ],
  });
  fillSlip(slip, bubble);
  return slip;
}

function fillSlip(slip: HTMLElement, bubble: BubbleModel): void {
  slip.dataset.kind = bubble.kind;
  slip.dataset.tail = bubble.tail;
  slip.dataset.speaker = bubble.speaker;
  setText(mustChild(slip, ".widget-name"), bubble.displayName);
  setText(mustChild(slip, ".widget-heading"), bubble.heading);
  const time = mustChild(slip, ".widget-time");
  time.hidden = bubble.timestamp.length === 0;
  setText(time, bubble.timestamp);
  const extracted = mustChild(slip, ".widget-extracted");
  extracted.hidden = bubble.extractedLabel == null;
  setText(extracted, bubble.extractedLabel ?? "");
  setText(mustChild(slip, ".widget-excerpt"), bubble.excerpt);
}

function mustChild(slip: HTMLElement, selector: string): HTMLElement {
  const node = slip.querySelector(selector);
  if (!(node instanceof HTMLElement)) {
    throw new Error(`Missing widget slip child ${selector}`);
  }
  return node;
}

function decorativeMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches === false;
  } catch {
    return true;
  }
}

function poke(figure: HTMLImageElement): void {
  if (!decorativeMotion()) {
    return;
  }
  figure.classList.remove("is-reacting");
  void figure.offsetWidth;
  figure.classList.add("is-reacting");
}
