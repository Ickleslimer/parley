import type { ViewerStatus, WidgetSnapshot } from "../contracts";
import type { ViewerApi } from "../ipc";

import { el, setText } from "./dom";
import type { PresentedMessage } from "./excerpt";
import { createSingleFlightPoller, WIDGET_POLL_MS } from "./poll";
import { widgetModel, type WidgetModel } from "./status-model";

export function mountWidget(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  root.className = "widget-shell";
  root.setAttribute("role", "status");
  root.setAttribute("aria-live", "polite");
  root.setAttribute("aria-atomic", "true");

  const banner = el("p", { className: "widget-banner" });
  banner.hidden = true;
  const logo = el("img", {
    className: "widget-logo",
    attrs: {
      src: "/parley-icon.ico",
      alt: "",
      width: "16",
      height: "16",
    },
  });
  logo.setAttribute("aria-hidden", "true");
  const title = el("h1", { className: "widget-title", text: "Parley" });
  const live = el("p", { className: "widget-live", text: "Live relay" });
  const brand = el("div", { className: "widget-brand", children: [title, live] });
  const participants = el("p", { className: "widget-participants" });
  participants.hidden = true;
  const header = el("header", {
    className: "widget-header",
    children: [logo, brand, participants],
  });
  const loadError = el("p", { className: "widget-error" });
  loadError.hidden = true;
  const idle = el("p", { className: "widget-idle", text: "Connecting\u2026" });
  const requestBlock = createMessageBlock("widget-request");
  const completionBlock = createMessageBlock("widget-completion");
  const thread = el("div", {
    className: "widget-thread",
    children: [requestBlock.root, completionBlock.root],
  });
  const source = el("p", { className: "widget-source", text: "Connecting\u2026" });
  const footer = el("footer", { className: "widget-footer", children: [source] });

  root.replaceChildren(banner, header, loadError, idle, thread, footer);

  let status: ViewerStatus | null = null;
  let snapshot: WidgetSnapshot | null = null;
  let error: string | null = null;
  let alive = true;

  const paint = (): void => {
    const model = widgetModel({ status, snapshot, loadError: error });
    paintBanner(banner, model.banner);
    setText(source, model.sourceLabel);
    paintParticipants(participants, model);
    if (model.loadError) {
      loadError.hidden = false;
      setText(loadError, model.loadError);
    } else {
      loadError.hidden = true;
      setText(loadError, "");
    }
    const hasConversation = Boolean(model.request || model.completion || model.pendingLabel);
    const showIdle = Boolean(model.idleLabel) && !hasConversation;
    idle.hidden = !showIdle;
    setText(idle, showIdle && model.idleLabel ? model.idleLabel : "");
    thread.hidden = !hasConversation;
    requestBlock.paint(model.request);
    paintCompletion(completionBlock, model);
  };

  const poller = createSingleFlightPoller(async () => {
    try {
      const [nextStatus, nextSnapshot] = await Promise.all([
        api.getStatus(),
        api.getWidgetSnapshot(),
      ]);
      if (!alive) {
        return;
      }
      status = nextStatus;
      snapshot = nextSnapshot;
      error = null;
    } catch {
      if (!alive) {
        return;
      }
      error = "Unable to load widget snapshot";
    }
    paint();
  }, WIDGET_POLL_MS);

  paint();
  poller.start();

  const stop = (): void => {
    alive = false;
    poller.stop();
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function paintParticipants(node: HTMLParagraphElement, model: WidgetModel): void {
  const pair = model.request
    ? [model.request.speaker, model.request.recipient]
    : model.completion
      ? [model.completion.recipient, model.completion.speaker]
      : null;
  if (!pair || pair.some((participant) => participant.length === 0)) {
    node.hidden = true;
    setText(node, "");
    return;
  }
  node.hidden = false;
  setText(node, `${pair[0]} \u2194 ${pair[1]}`);
}

function paintBanner(node: HTMLParagraphElement, text: string | null): void {
  if (text) {
    node.hidden = false;
    setText(node, text);
  } else {
    node.hidden = true;
    setText(node, "");
  }
}

function paintCompletion(
  block: ReturnType<typeof createMessageBlock>,
  model: WidgetModel,
): void {
  if (model.completion) {
    block.paint(model.completion);
    return;
  }
  if (model.pendingLabel) {
    block.paintPending(model.pendingLabel);
    return;
  }
  block.paint(null);
}

function createMessageBlock(className: string) {
  const avatar = el("span", {
    className: "widget-avatar",
    attrs: { "aria-hidden": "true" },
  });
  const name = el("h2", { className: "widget-name" });
  const address = el("p", { className: "widget-address" });
  const meta = el("p", { className: "widget-meta" });
  const extracted = el("p", { className: "widget-extracted" });
  extracted.hidden = true;
  const excerpt = el("p", { className: "widget-excerpt" });
  const identity = el("div", {
    className: "widget-identity",
    children: [name, address, meta],
  });
  const bubble = el("div", {
    className: "widget-bubble",
    children: [identity, extracted, excerpt],
  });
  const root = el("section", {
    className: `widget-message ${className}`,
    children: [avatar, bubble],
  });
  root.hidden = true;

  return {
    root,
    paint(message: PresentedMessage | null) {
      if (!message) {
        root.hidden = true;
        root.classList.remove("is-error", "is-pending");
        setText(excerpt, "");
        return;
      }
      const error = message.eventType === "error";
      root.hidden = false;
      root.classList.toggle("is-error", error);
      root.classList.remove("is-pending");
      avatar.hidden = false;
      setText(avatar, error ? "!" : message.initial);
      name.hidden = false;
      setText(name, error ? message.heading : message.speaker);
      if (error || message.recipient.length === 0) {
        address.hidden = true;
        setText(address, "");
      } else {
        address.hidden = false;
        setText(address, `to ${message.recipient}`);
      }
      setText(meta, message.timestamp);
      if (message.extractedLabel) {
        extracted.hidden = false;
        setText(extracted, message.extractedLabel);
      } else {
        extracted.hidden = true;
        setText(extracted, "");
      }
      setText(excerpt, message.excerpt);
    },
    paintPending(label: string) {
      root.hidden = false;
      root.classList.remove("is-error");
      root.classList.add("is-pending");
      avatar.hidden = false;
      setText(avatar, "");
      name.hidden = true;
      setText(name, "");
      address.hidden = true;
      setText(address, "");
      setText(meta, "");
      extracted.hidden = true;
      setText(extracted, "");
      setText(excerpt, label);
    },
  };
}
