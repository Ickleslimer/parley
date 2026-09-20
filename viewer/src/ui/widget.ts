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
      width: "22",
      height: "22",
    },
  });
  logo.setAttribute("aria-hidden", "true");
  const title = el("h1", { className: "widget-title", text: "Parley" });
  const source = el("p", { className: "widget-source", text: "Connecting\u2026" });
  const header = el("header", { className: "widget-header", children: [logo, title, source] });
  const loadError = el("p", { className: "widget-error" });
  loadError.hidden = true;
  const idle = el("p", { className: "widget-idle", text: "Connecting\u2026" });
  const requestBlock = createMessageBlock("widget-request");
  const completionBlock = createMessageBlock("widget-completion");

  root.replaceChildren(banner, header, loadError, idle, requestBlock.root, completionBlock.root);

  let status: ViewerStatus | null = null;
  let snapshot: WidgetSnapshot | null = null;
  let error: string | null = null;
  let alive = true;

  const paint = (): void => {
    const model = widgetModel({ status, snapshot, loadError: error });
    paintBanner(banner, model.banner);
    setText(source, model.sourceLabel);
    if (model.loadError) {
      loadError.hidden = false;
      setText(loadError, model.loadError);
    } else {
      loadError.hidden = true;
      setText(loadError, "");
    }
    const showIdle = Boolean(model.idleLabel) && !model.request && !model.completion && !model.pendingLabel;
    idle.hidden = !showIdle;
    setText(idle, showIdle && model.idleLabel ? model.idleLabel : "");
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
  const heading = el("h2", { className: "widget-heading" });
  const meta = el("p", { className: "widget-meta" });
  const extracted = el("p", { className: "widget-extracted" });
  extracted.hidden = true;
  const excerpt = el("p", { className: "widget-excerpt" });
  const root = el("section", {
    className: `widget-message ${className}`,
    children: [heading, meta, extracted, excerpt],
  });
  root.hidden = true;

  return {
    root,
    paint(message: PresentedMessage | null) {
      if (!message) {
        root.hidden = true;
        setText(excerpt, "");
        return;
      }
      root.hidden = false;
      setText(heading, message.heading);
      setText(meta, `${message.route} \u00b7 ${message.timestamp}`);
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
      setText(heading, "Pending");
      setText(meta, "");
      extracted.hidden = true;
      setText(extracted, "");
      setText(excerpt, label);
    },
  };
}
