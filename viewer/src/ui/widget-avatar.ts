import { AVATAR_ILLUSTRATIONS } from "./assets";
import { el } from "./dom";

export type WidgetAvatarAgent = "codex" | "grok" | "neutral";

export interface WidgetAvatarState {
  agent: WidgetAvatarAgent;
  working: boolean;
}

const MOTION_CLASS = "widget-avatar-working";

function knownAgent(agent: unknown): "codex" | "grok" | null {
  return agent === "codex" || agent === "grok" ? agent : null;
}

function portrait(agent: "codex" | "grok", working: boolean): HTMLImageElement {
  const asset =
    agent === "codex"
      ? working
        ? AVATAR_ILLUSTRATIONS.codexWorking
        : AVATAR_ILLUSTRATIONS.codexIdle
      : working
        ? AVATAR_ILLUSTRATIONS.grokWorking
        : AVATAR_ILLUSTRATIONS.grokIdle;
  const image = el("img", {
    className: working ? `widget-feed-avatar ${MOTION_CLASS}` : "widget-feed-avatar",
    attrs: {
      src: asset.path,
      alt: "",
      decoding: "async",
      draggable: "false",
    },
  });
  image.draggable = false;
  image.addEventListener("error", (event) => {
    event.stopPropagation();
    image.classList.add("is-missing");
  });
  return image;
}

function neutralDevice(): HTMLSpanElement {
  return el("span", {
    className: "widget-feed-device",
    children: [el("span", { className: "widget-feed-device-screen" })],
  });
}

export function createWidgetAvatar(state: WidgetAvatarState): HTMLElement {
  const agent = knownAgent(state?.agent);
  const working = agent !== null && state?.working === true;
  return el("span", {
    className: "widget-feed-avatar-slot",
    attrs: {
      "data-agent": agent ?? "neutral",
      "data-working": working ? "true" : "false",
      "aria-hidden": "true",
    },
    children: [agent === null ? neutralDevice() : portrait(agent, working)],
  });
}
