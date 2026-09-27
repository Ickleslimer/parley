import type { PeerActivitySnapshot, PeerHealthSnapshot, ViewerSettings } from "../contracts";
import { isIncidentClass } from "../contracts";

import { el, labelledControl } from "./dom";
import { isFocusLocked, isTimelineKey, type TimelineKey } from "./timeline";
import { LANDMARKS } from "./landmarks";

export const STUDIO_TABS = ["event", "activity", "sources", "settings"] as const;
export type StudioTab = (typeof STUDIO_TABS)[number];

const TAB_LABELS: Record<StudioTab, string> = {
  event: "Event",
  activity: "Activity",
  sources: "Sources",
  settings: "Settings",
};

const TAB_REGIONS: Record<StudioTab, string> = {
  event: LANDMARKS.studioTabEvent,
  activity: LANDMARKS.studioTabActivity,
  sources: LANDMARKS.studioTabSources,
  settings: LANDMARKS.studioTabSettings,
};

const CORNERS: Array<ViewerSettings["corner"]> = [
  "top-left",
  "top-right",
  "bottom-left",
  "bottom-right",
];

const CORNER_LABELS: Record<ViewerSettings["corner"], string> = {
  "top-left": "Top left",
  "top-right": "Top right",
  "bottom-left": "Bottom left",
  "bottom-right": "Bottom right",
};

export function unreadIncidentCount(snapshot: PeerHealthSnapshot | null): number {
  if (!snapshot) {
    return 0;
  }
  return snapshot.activeIncidents.filter(
    (incident) => isIncidentClass(incident.class) && !incident.acknowledged,
  ).length;
}

export function pendingHandoffCount(snapshot: PeerActivitySnapshot | null): number {
  if (!snapshot) {
    return 0;
  }
  return snapshot.handoffs.filter(
    (handoff) => handoff.receiptAtMs == null && handoff.state !== "acknowledged",
  ).length;
}

export function activityAttentionLabel(unread: number, pending: number): string {
  return `${count(unread)} unread, ${count(pending)} pending`;
}

export interface InspectorNodes {
  region: HTMLElement;
  tablist: HTMLElement;
  tabs: Record<StudioTab, HTMLButtonElement>;
  panels: Record<StudioTab, HTMLElement>;
  activityCount: HTMLElement;
  selected: () => StudioTab;
  activate: (tab: StudioTab) => void;
  setActivityAttention: (label: string) => void;
  eventMeta: HTMLDListElement;
  eventEmpty: HTMLParagraphElement;
  eventBody: HTMLPreElement;
  activityHost: HTMLElement;
  sourceList: HTMLUListElement;
  sourceEmpty: HTMLParagraphElement;
  diagnostics: HTMLParagraphElement;
  selectLog: HTMLButtonElement;
  monitor: HTMLSelectElement;
  corner: HTMLSelectElement;
  offsetX: HTMLInputElement;
  offsetY: HTMLInputElement;
  width: HTMLInputElement;
  height: HTMLInputElement;
  saveSettings: HTMLButtonElement;
  settingsForm: HTMLFormElement;
  widgetVisible: HTMLInputElement;
  launchAtLogin: HTMLInputElement;
}

export function buildInspector(): InspectorNodes {
  const tabs = {} as Record<StudioTab, HTMLButtonElement>;
  const panels = {} as Record<StudioTab, HTMLElement>;
  let activityCount = el("span");
  for (const tab of STUDIO_TABS) {
    const count = tab === "activity" ? el("span", { className: "studio-tab-count", text: activityAttentionLabel(0, 0) }) : null;
    if (count) {
      activityCount = count;
    }
    const button = el("button", {
      className: "studio-tab",
      attrs: {
        type: "button",
        role: "tab",
        id: TAB_REGIONS[tab],
        "data-region": TAB_REGIONS[tab],
        "aria-controls": `studio-panel-${tab}`,
        "aria-selected": tab === "event" ? "true" : "false",
        tabindex: tab === "event" ? "0" : "-1",
      },
      children: [el("span", { className: "studio-tab-name", text: TAB_LABELS[tab] }), count],
    });
    tabs[tab] = button;
    const panel = el("div", {
      className: "studio-panel",
      attrs: {
        role: "tabpanel",
        id: `studio-panel-${tab}`,
        "aria-labelledby": TAB_REGIONS[tab],
      },
    });
    panel.hidden = tab !== "event";
    panels[tab] = panel;
  }

  const eventEmpty = el("p", { className: "studio-empty", text: "Select a request, completion, or search hit" });
  const eventMeta = el("dl", { className: "studio-event-meta" });
  const eventBody = el("pre", {
    className: "event-body exact-body",
    attrs: { tabindex: "0", "aria-label": "Exact event content" },
  });
  panels.event.append(eventEmpty, eventMeta, eventBody);

  const activityHost = el("div", { className: "studio-activity-host" });
  panels.activity.append(activityHost);

  const selectLog = el("button", {
    className: "studio-action",
    text: "Add log",
    attrs: { type: "button", "aria-describedby": "source-status" },
  });
  const sourceEmpty = el("p", { className: "studio-empty", text: "No event logs configured" });
  const sourceList = el("ul", {
    className: "studio-source-list",
    attrs: { "aria-label": "Configured event logs" },
  });
  const diagnostics = el("p", { className: "studio-diagnostics" });
  panels.sources.append(
    el("h3", { className: "studio-panel-title", text: "Event logs" }),
    selectLog,
    sourceEmpty,
    sourceList,
    el("h3", { className: "studio-panel-title", text: "Diagnostics" }),
    diagnostics,
  );

  const monitor = el("select", { attrs: { id: "monitor", "aria-label": "Monitor" } });
  const corner = el("select", { attrs: { id: "corner", "aria-label": "Corner" } });
  for (const value of CORNERS) {
    corner.append(el("option", { text: CORNER_LABELS[value], attrs: { value } }));
  }
  const offsetX = numberInput("offset-x", "Horizontal offset");
  const offsetY = numberInput("offset-y", "Vertical offset");
  const width = numberInput("width", "Width");
  const height = numberInput("height", "Height");
  const widgetVisible = el("input", { attrs: { type: "checkbox", id: "widget-visible" } });
  const launchAtLogin = el("input", { attrs: { type: "checkbox", id: "launch-at-login" } });
  const saveSettings = el("button", {
    className: "studio-action",
    text: "Save placement",
    attrs: { type: "submit" },
  });
  const settingsForm = el("form", {
    className: "studio-settings",
    children: [
      el("h3", { className: "studio-panel-title", text: "Widget placement" }),
      labelledControl("Monitor", monitor),
      labelledControl("Corner", corner),
      labelledControl("Horizontal offset", offsetX),
      labelledControl("Vertical offset", offsetY),
      labelledControl("Width", width),
      labelledControl("Height", height),
      saveSettings,
    ],
  });
  panels.settings.append(
    settingsForm,
    el("div", {
      className: "studio-settings-runtime",
      children: [
        labelledControl("Widget visible", widgetVisible, "field field-check"),
        labelledControl("Launch at login", launchAtLogin, "field field-check"),
      ],
    }),
  );

  const tablist = el("div", {
    className: "studio-tablist",
    attrs: { role: "tablist", "aria-label": "Inspector", "aria-orientation": "horizontal" },
    children: STUDIO_TABS.map((tab) => tabs[tab]),
  });
  const region = el("section", {
    className: "studio-inspector",
    attrs: { "data-region": LANDMARKS.studioInspector, "aria-label": "Inspector" },
    children: [tablist, ...STUDIO_TABS.map((tab) => panels[tab])],
  });

  let selected: StudioTab = "event";
  let focused: StudioTab = "event";

  const paint = (): void => {
    for (const tab of STUDIO_TABS) {
      const active = tab === selected;
      tabs[tab].setAttribute("aria-selected", active ? "true" : "false");
      tabs[tab].tabIndex = tab === focused ? 0 : -1;
      panels[tab].hidden = !active;
    }
  };

  const focusTab = (tab: StudioTab): void => {
    focused = tab;
    paint();
    tabs[tab].focus();
  };

  const activate = (tab: StudioTab): void => {
    selected = tab;
    focused = tab;
    paint();
  };

  for (const tab of STUDIO_TABS) {
    tabs[tab].addEventListener("click", () => activate(tab));
    tabs[tab].addEventListener("focus", () => {
      focused = tab;
      for (const item of STUDIO_TABS) {
        tabs[item].tabIndex = item === tab ? 0 : -1;
      }
    });
  }

  tablist.addEventListener("keydown", (event) => {
    if (isFocusLocked(event.target)) {
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activate(focused);
      return;
    }
    if (!isTimelineKey(event.key)) {
      return;
    }
    event.preventDefault();
    const next = moveTab(focused, event.key);
    focusTab(next);
  });

  tablist.addEventListener("focusout", (event) => {
    const next = event.relatedTarget;
    if (next instanceof Node && tablist.contains(next)) {
      return;
    }
    focused = selected;
    for (const tab of STUDIO_TABS) {
      tabs[tab].tabIndex = tab === selected ? 0 : -1;
    }
  });

  return {
    region,
    tablist,
    tabs,
    panels,
    activityCount,
    selected: () => selected,
    activate,
    setActivityAttention: (label: string) => {
      if (activityCount.textContent !== label) {
        activityCount.textContent = label;
      }
    },
    eventMeta,
    eventEmpty,
    eventBody,
    activityHost,
    sourceList,
    sourceEmpty,
    diagnostics,
    selectLog,
    monitor,
    corner,
    offsetX,
    offsetY,
    width,
    height,
    saveSettings,
    settingsForm,
    widgetVisible,
    launchAtLogin,
  };
}

function moveTab(current: StudioTab, key: TimelineKey): StudioTab {
  const index = STUDIO_TABS.indexOf(current);
  if (key === "Home") {
    return "event";
  }
  if (key === "End") {
    return "settings";
  }
  if (key === "ArrowRight" || key === "ArrowDown") {
    return STUDIO_TABS[Math.min(STUDIO_TABS.length - 1, index + 1)] ?? current;
  }
  if (key === "ArrowLeft" || key === "ArrowUp") {
    return STUDIO_TABS[Math.max(0, index - 1)] ?? current;
  }
  return current;
}

function numberInput(id: string, label: string): HTMLInputElement {
  return el("input", {
    attrs: { id, type: "number", "aria-label": label },
  });
}

function count(value: number): number {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.trunc(value));
}
