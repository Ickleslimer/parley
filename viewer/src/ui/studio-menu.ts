import { el } from "./dom";
import { LANDMARKS } from "./landmarks";
import { UTILITY_IDS, type StudioMenuNodes, type UtilityId } from "./studio-seam";

const UTILITY_LABELS: Record<UtilityId, string> = {
  activity: "Activity",
  sources: "Sources",
  settings: "Settings",
};

let menuSequence = 0;

export function createStudioMenu(onChoose: (utility: UtilityId) => void): StudioMenuNodes {
  const menuId = `studio-utilities-menu-${(menuSequence += 1)}`;
  const count = el("span", {
    className: "studio-focus-utility-count",
    attrs: { "aria-hidden": "true" },
  });
  const button = el("button", {
    className: "studio-focus-utilities-button",
    attrs: {
      type: "button",
      "data-region": LANDMARKS.studioUtilities,
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      "aria-controls": menuId,
    },
    children: [el("span", { className: "studio-focus-utility-label", text: "Utilities" }), count],
  });
  const items = {
    activity: menuItem("activity"),
    sources: menuItem("sources"),
    settings: menuItem("settings"),
  } satisfies Record<UtilityId, HTMLButtonElement>;
  const order = UTILITY_IDS.map((id) => items[id]);
  const menu = el("div", {
    className: "studio-focus-menu",
    attrs: { id: menuId, role: "menu", "aria-label": "Utilities" },
    children: order,
  });
  menu.hidden = true;

  const close = (restoreFocus = false): void => {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    if (restoreFocus) {
      button.focus();
    }
  };

  const open = (edge: "first" | "last"): void => {
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    focusItem(edge === "last" ? items.settings : items.activity);
  };

  const choose = (utility: UtilityId): void => {
    onChoose(utility);
    close(true);
  };

  button.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " " || event.key === "Home") {
      event.preventDefault();
      open("first");
      return;
    }
    if (event.key === "ArrowUp" || event.key === "End") {
      event.preventDefault();
      open("last");
      return;
    }
    if (event.key === "Escape" && !menu.hidden) {
      event.preventDefault();
      close(true);
    }
  });
  button.addEventListener("click", () => {
    if (menu.hidden) {
      open("first");
    } else {
      close(false);
    }
  });

  for (const item of order) {
    item.addEventListener("click", () => {
      const utility = utilityFrom(item);
      if (utility) {
        choose(utility);
      }
    });
  }

  menu.addEventListener("keydown", (event) => {
    const current = order.find((item) => item === document.activeElement) ?? null;
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if ((event.key === "Enter" || event.key === " ") && current) {
      const utility = utilityFrom(current);
      if (!utility) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      current.click();
      return;
    }
    if (!current) {
      return;
    }
    const next = moveItem(order, current, event.key);
    if (!next) {
      return;
    }
    event.preventDefault();
    focusItem(next);
  });

  return { button, menu, items, close };
}

function menuItem(utility: UtilityId): HTMLButtonElement {
  return el("button", {
    className: "studio-focus-menu-item",
    text: UTILITY_LABELS[utility],
    attrs: {
      type: "button",
      role: "menuitem",
      tabindex: "-1",
      "data-utility": utility,
    },
  });
}

function utilityFrom(item: HTMLButtonElement): UtilityId | null {
  const value = item.dataset.utility;
  return UTILITY_IDS.find((id) => id === value) ?? null;
}

function focusItem(target: HTMLButtonElement): void {
  const menu = target.parentElement;
  if (!(menu instanceof HTMLElement)) {
    target.focus();
    return;
  }
  for (const item of menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')) {
    item.tabIndex = item === target ? 0 : -1;
  }
  target.focus();
}

function moveItem(
  order: readonly HTMLButtonElement[],
  current: HTMLButtonElement,
  key: string,
): HTMLButtonElement | null {
  const index = order.indexOf(current);
  if (index < 0) {
    return null;
  }
  if (key === "Home") {
    return order[0] ?? null;
  }
  if (key === "End") {
    return order[order.length - 1] ?? null;
  }
  if (key === "ArrowDown") {
    return order[Math.min(order.length - 1, index + 1)] ?? null;
  }
  if (key === "ArrowUp") {
    return order[Math.max(0, index - 1)] ?? null;
  }
  return null;
}
