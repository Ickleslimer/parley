import { ILLUSTRATIONS } from "./assets";
import type { BubbleModel, WidgetSceneModel } from "./attribution";
import { el, setText } from "./dom";
import { LANDMARKS } from "./landmarks";

export interface WidgetSceneHandle {
  paint(model: WidgetSceneModel): void;
}

export function createWidgetScene(root: HTMLElement): WidgetSceneHandle {
  root.className = "widget-shell";
  root.removeAttribute("role");
  root.removeAttribute("aria-live");
  root.removeAttribute("aria-atomic");
  root.removeAttribute("tabindex");

  const plate = decorativeImage("widget-plate", ILLUSTRATIONS.plate.path);
  const codex = decorativeImage(
    `widget-figure widget-figure-codex`,
    ILLUSTRATIONS.codex.path,
    LANDMARKS.widgetCodex,
  );
  const grok = decorativeImage(
    `widget-figure widget-figure-grok`,
    ILLUSTRATIONS.grok.path,
    LANDMARKS.widgetGrok,
  );
  const banner = el("p", { className: "widget-banner" });
  banner.hidden = true;
  const loadError = el("p", { className: "widget-load-error" });
  loadError.hidden = true;
  const idle = el("p", { className: "widget-idle" });
  const messages = el("div", { className: "widget-messages" });
  const live = el("div", {
    id: LANDMARKS.widgetLive,
    className: "widget-live",
    attrs: {
      role: "status",
      "aria-live": "polite",
      "aria-atomic": "true",
    },
    children: [banner, loadError, idle, messages],
  });
  const source = el("p", { className: "widget-source" });
  const column = el("div", {
    className: "widget-column",
    children: [live, source],
  });
  const bench = el("div", {
    className: "widget-bench",
    children: [codex, column, grok],
  });
  const scene = el("section", {
    id: LANDMARKS.widgetScene,
    className: "widget-scene",
    children: [plate, bench],
  });
  root.replaceChildren(scene);

  return {
    paint(model) {
      paintNotice(banner, model.banner);
      setText(source, model.sourceLabel);
      paintNotice(loadError, model.loadError);
      const showIdle = Boolean(model.idleLabel) && model.bubbles.length === 0 && !model.loadError;
      idle.hidden = !showIdle;
      setText(idle, showIdle && model.idleLabel ? model.idleLabel : "");
      syncBubbles(messages, model.bubbles, codex, grok);
    },
  };
}

function decorativeImage(className: string, src: string, id?: string): HTMLImageElement {
  const image = el("img", {
    className,
    ...(id ? { id } : {}),
    attrs: {
      src,
      alt: "",
      "aria-hidden": "true",
      decoding: "async",
      draggable: "false",
    },
  });
  image.addEventListener("error", () => {
    image.classList.add("is-missing");
  });
  return image;
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
  codex: HTMLImageElement,
  grok: HTMLImageElement,
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
  if (reactCodex) {
    poke(codex);
  }
  if (reactGrok) {
    poke(grok);
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
