import type { ViewerApi } from "../ipc";
import { mountDetail } from "../ui/detail";
import { mountWidget } from "../ui/widget";
import { mountWidgetSurface } from "../ui/widget-surface";
import {
  type FixtureInspectorTab,
  type FixtureScenario,
  type FixtureSurface,
} from "./contracts";
import { createSyntheticFixture } from "./synthetic-api";

const SCENARIOS = new Set<FixtureScenario>([
  "short-exchange",
  "maximum-exchange",
  "reversed-route",
  "unknown-agent",
  "pending",
  "error",
  "idle",
  "missing-image",
  "search-results",
  "empty",
  "source-error",
  "live",
  "passive-fallback",
  "paused-unread",
  "collapsed",
  "expanded",
  "completion",
  "handoff-preview",
]);

export function mountSyntheticFixture(
  root: HTMLElement,
  _realApi: ViewerApi,
  surface: FixtureSurface,
  rawScenario: string,
): void {
  const scenario = SCENARIOS.has(rawScenario as FixtureScenario)
    ? (rawScenario as FixtureScenario)
    : "short-exchange";
  const params = new URLSearchParams(window.location.search);
  const fixture = createSyntheticFixture(scenario);
  if (params.get("attention") === "none") {
    const readHealth = fixture.api.getPeerHealth;
    const readActivity = fixture.api.getPeerActivity;
    fixture.api.getPeerHealth = async () => ({
      ...await readHealth(),
      activeIncidents: [],
      unreadCount: 0,
    });
    fixture.api.getPeerActivity = async () => ({
      ...await readActivity(),
      handoffs: [],
    });
  }
  if (params.get("continuation") === "reviewed") {
    const readStatus = fixture.api.getContinuationStatus;
    fixture.api.getContinuationStatus = async () => ({
      ...await readStatus(),
      state: "reviewed",
      manifestId: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      worktreeName: "Synthetic task",
      writablePaths: ["src/synthetic-task.ts"],
      expiresAtMs: 1_900_000_000_000,
      continuationCount: 1,
      latest: {
        schemaVersion: 1,
        continuationId: "11111111-2222-4333-8444-555555555555",
        manifestId: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        state: "succeeded",
        prompt: "Synthetic task request.",
        reply: "Synthetic task completed.",
        error: null,
        exchangeId: "synthetic-direct-exchange",
        requestEventId: "synthetic-direct-request",
        completionEventId: "synthetic-direct-response",
        changedPaths: ["src/synthetic-task.ts"],
        validationState: "not_executed",
        diffFingerprint: "a".repeat(64),
        evidenceHash: "b".repeat(64),
        completedAtMs: 1_800_000_000_000,
        userReceived: true,
      },
    });
  }
  const fixtureWidth = positiveFixtureDimension(params.get("fixture-width"));
  const fixtureHeight = positiveFixtureDimension(params.get("fixture-height"));
  if (fixtureWidth && fixtureHeight) {
    root.style.width = `${fixtureWidth}px`;
    root.style.height = `${fixtureHeight}px`;
  }
  document.title = `Synthetic ${surface} fixture`;
  document.documentElement.dataset.syntheticFixture = scenario;
  document.documentElement.classList.toggle("fixture-missing-images", scenario === "missing-image");
  if (params.get("text-scale") === "2") {
    document.documentElement.style.fontSize = "200%";
  }
  const label = document.createElement("p");
  label.className = "synthetic-fixture-label";
  label.textContent = `Synthetic fixture: ${scenario}`;
  const style = document.createElement("style");
  style.textContent = `.synthetic-fixture-label{position:fixed;z-index:9999;right:8px;bottom:8px;margin:0;padding:3px 6px;border:1px solid #241c16;background:#f3e6d0;color:#241c16;font:12px/1.2 sans-serif}.fixture-missing-images img{display:none!important}`;
  document.head.append(style);
  document.body.append(label);
  if (surface === "widget") {
    mountWidget(root, fixture.api);
  } else if (surface === "widget-surface") {
    mountWidgetSurface(root, fixture.api);
  } else {
    mountDetail(root, fixture.api);
    void prepareDetailFixture(root, scenario, params.get("tab"));
    if (params.get("menu") === "open") {
      root.querySelector<HTMLButtonElement>('[data-region="studio-utilities"]')?.click();
    }
    if (params.get("composer") === "open") {
      const composer = root.querySelector<HTMLDetailsElement>(".continuation-dock");
      const message = root.querySelector<HTMLTextAreaElement>(".continuation-input");
      if (composer && message) {
        composer.open = true;
        message.value = "Synthetic draft for Grok.";
        message.dispatchEvent(new Event("input", { bubbles: true }));
      }
    }
  }
  if (scenario === "missing-image") {
    queueMicrotask(() => {
      for (const image of root.querySelectorAll<HTMLImageElement>("img")) {
        image.hidden = true;
      }
    });
  }
}

function positiveFixtureDimension(value: string | null): number | null {
  if (!value || !/^\d+$/.test(value)) {
    return null;
  }
  const parsed = Number.parseInt(value, 10);
  return parsed >= 1 && parsed <= 4_096 ? parsed : null;
}

async function prepareDetailFixture(
  root: HTMLElement,
  scenario: FixtureScenario,
  tab: string | null,
): Promise<void> {
  if (scenario !== "empty" && scenario !== "source-error") {
    if (scenario === "search-results") {
      const input = await waitForElement<HTMLInputElement>(root, 'input[type="search"]');
      if (input) {
        input.value = "synthetic evidence";
        input.form?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
        await waitForElement(root, '.studio-focus-row[data-event-key]');
      }
    }
  }
  await activateStudioSurface(root, tab, scenario === "maximum-exchange" || scenario === "handoff-preview");
}

async function waitForElement<T extends Element>(
  root: ParentNode,
  selector: string,
): Promise<T | null> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const element = root.querySelector<T>(selector);
    if (element) {
      return element;
    }
    await new Promise<void>((resolve) => window.setTimeout(resolve, 25));
  }
  return null;
}

async function activateStudioSurface(
  root: HTMLElement,
  value: string | null,
  selectCompletion = false,
): Promise<void> {
  if (value !== "event" && value !== "activity" && value !== "sources" && value !== "settings") {
    return;
  }
  if (value === "event") {
    const message = await waitForElement<HTMLButtonElement>(
      root,
      ".studio-focus-row[data-event-key]",
    );
    const selection = selectCompletion
      ? root.querySelectorAll<HTMLButtonElement>(".studio-focus-row[data-event-key]")[1]
      : message;
    selection?.click();
    return;
  }
  const utilities = await waitForElement<HTMLButtonElement>(
    root,
    '[data-region="studio-utilities"]',
  );
  utilities?.click();
  const item = await waitForElement<HTMLButtonElement>(
    root,
    `[role="menuitem"][data-utility="${value}"]`,
  );
  item?.click();
}
