import { el, setText } from "./dom";
import { formatCount } from "./format";
import { LANDMARKS } from "./landmarks";
import { createStudioMenu } from "./studio-menu";
import type { StudioHeaderNodes, StudioHeaderPaint, UtilityId } from "./studio-seam";

export function createStudioHeader(handlers: {
  onExit: () => void;
  onUtility: (utility: UtilityId) => void;
}): StudioHeaderNodes {
  const status = el("p", {
    className: "studio-focus-status",
    attrs: { "data-region": LANDMARKS.studioStatusLine },
  });
  const banner = el("p", {
    className: "studio-focus-banner",
    attrs: { "data-region": LANDMARKS.studioBanner },
  });
  banner.hidden = true;
  const utilities = createStudioMenu(handlers.onUtility);
  const exit = el("button", {
    className: "studio-focus-exit",
    text: "Exit",
    attrs: { type: "button", "data-region": LANDMARKS.studioExit },
  });
  exit.addEventListener("click", () => {
    handlers.onExit();
  });
  const region = el("header", {
    className: "studio-focus-header",
    attrs: { "data-region": LANDMARKS.studioHeader },
    children: [
      el("div", {
        className: "studio-focus-brand",
        children: [el("h1", { text: "Conversation studio" }), status],
      }),
      banner,
      el("div", {
        className: "studio-focus-utilities",
        children: [utilities.button, utilities.menu],
      }),
      exit,
    ],
  });

  return {
    region,
    status,
    banner,
    utilities,
    exit,
    paint: (input) => paintHeader(status, banner, utilities.button, input),
  };
}

function paintHeader(
  status: HTMLParagraphElement,
  banner: HTMLParagraphElement,
  utilitiesButton: HTMLButtonElement,
  input: StudioHeaderPaint,
): void {
  setText(status, input.status);
  if (input.banner === null) {
    banner.hidden = true;
    setText(banner, "");
  } else {
    banner.hidden = false;
    setText(banner, input.banner);
  }
  const count = utilitiesButton.querySelector(".studio-focus-utility-count");
  if (!(count instanceof HTMLElement)) {
    return;
  }
  const label = utilityCount(input.unreadCount, input.pendingCount);
  setText(count, label);
  if (label) {
    count.removeAttribute("aria-hidden");
  } else {
    count.setAttribute("aria-hidden", "true");
  }
}

function utilityCount(unreadCount: number, pendingCount: number): string {
  const parts: string[] = [];
  const unread = visibleCount(unreadCount);
  const pending = visibleCount(pendingCount);
  if (unread > 0) {
    parts.push(formatCount(unread, "unread", "unread"));
  }
  if (pending > 0) {
    parts.push(formatCount(pending, "pending", "pending"));
  }
  return parts.join(", ");
}

function visibleCount(value: number): number {
  if (!Number.isFinite(value) || value <= 0) {
    return 0;
  }
  return Math.trunc(value);
}
