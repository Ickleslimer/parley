import type { ViewerStatus, WidgetFeedExchange, WidgetFeedMessage, WidgetFeedPage } from "../contracts";
import type { ViewerApi } from "../ipc";

import { AVATAR_ILLUSTRATIONS } from "./assets";
import { displaySpeaker, normalizeSpeaker, tailForSpeaker, type TailDirection } from "./attribution";
import { el, setText } from "./dom";
import { formatTimestamp, idleWidgetLabel } from "./format";
import { degradedBanner, loadErrorLabel, PARLEY_ERROR_LABEL } from "./labels";
import { LANDMARKS } from "./landmarks";
import { createSingleFlightPoller, WIDGET_POLL_MS } from "./poll";

export const FEED_PAGE_EXCHANGES = 20;
export const FEED_MAX_EXCHANGES = 200;
export const LIVE_PAUSE_DISTANCE_PX = 24;

const HISTORY_LABEL = "History changed \u00b7 Jump to live";
const FULL_MESSAGE_ERROR = loadErrorLabel("load the full message");
const FEED_LOAD_ERROR = loadErrorLabel("load conversation");

export function distanceFromBottom(node: {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}): number {
  const distance = node.scrollHeight - node.scrollTop - node.clientHeight;
  return Number.isFinite(distance) ? distance : 0;
}

const pendingProgrammaticScrolls = new WeakMap<HTMLElement, number>();

export function assignProgrammaticScrollTop(node: HTMLElement, value: number): void {
  pendingProgrammaticScrolls.set(node, value);
  node.scrollTop = value;
}

function consumeProgrammaticScroll(node: HTMLElement): boolean {
  const expected = pendingProgrammaticScrolls.get(node);
  if (expected === undefined) {
    return false;
  }
  pendingProgrammaticScrolls.delete(node);
  return Math.abs(node.scrollTop - expected) <= 1;
}

export function userScrollShouldPause(input: {
  followingLive: boolean;
  programmatic: boolean;
  distanceFromBottomPx: number;
}): boolean {
  return (
    input.followingLive &&
    !input.programmatic &&
    input.distanceFromBottomPx > LIVE_PAUSE_DISTANCE_PX
  );
}

type ScrollHold = "pin" | "pin-if-moved" | "preserve" | "anchor" | "none";
type AvatarKind = "codex" | "grok" | "neutral" | "none";

interface StoredMessage {
  eventKey: string;
  eventType: WidgetFeedMessage["eventType"];
  speaker: string;
  recipient: string;
  timestampMs: number;
  status: string;
  automaticBody: string;
  fullBody: string | null;
  expanded: boolean;
  truncated: boolean;
  fullCharacterLength: number;
  projection: WidgetFeedMessage["projection"];
  contextOmitted: boolean;
  expansionError: string | null;
}

interface StoredExchange {
  exchangeKey: string;
  sessionKey: string;
  timestampMs: number;
  request: StoredMessage | null;
  completion: StoredMessage | null;
  pendingLabel: string | null;
}

interface FeedState {
  historyToken: string | null;
  exchanges: StoredExchange[];
  hasEarlier: boolean;
  nextBeforeExchangeKey: string | null;
  followingLive: boolean;
  unread: string[];
  historyChanged: boolean;
  loadError: string | null;
  status: ViewerStatus | null;
}

interface RowModel {
  key: string;
  eventKey: string | null;
  kind: "speech" | "parley-error" | "pending";
  speaker: "codex" | "grok" | "unknown";
  displayName: string;
  tail: TailDirection;
  avatar: AvatarKind;
  meta: string;
  body: string;
  note: string | null;
  error: string | null;
  showFull: boolean;
  expanded: boolean;
  projection: string;
}

interface RowHandle {
  root: HTMLDivElement;
  name: HTMLParagraphElement;
  meta: HTMLParagraphElement;
  body: HTMLParagraphElement;
  note: HTMLParagraphElement;
  error: HTMLParagraphElement;
  fullButton: HTMLButtonElement;
  openButton: HTMLButtonElement;
  avatar: HTMLElement | null;
}

export interface MountWidgetFeedOptions {
  parent: HTMLElement;
  api: ViewerApi;
  takePointerReassertion: () => Promise<void>;
  onPresented: (changed: boolean) => void;
  onPolled: (changed: boolean) => void;
}

export function mountWidgetFeed(options: MountWidgetFeedOptions): { stop: () => void } {
  const capturePresentation = readCapturePresentation();
  const state: FeedState = {
    historyToken: null,
    exchanges: [],
    hasEarlier: false,
    nextBeforeExchangeKey: null,
    followingLive: true,
    unread: [],
    historyChanged: false,
    loadError: null,
    status: null,
  };
  const exchangeNodes = new Map<string, HTMLElement>();
  const rowNodes = new Map<string, RowHandle>();
  const dividerNodes = new Map<string, HTMLParagraphElement>();
  const expanding = new Set<string>();
  let paintedList: string | null = null;
  let paintedRevision: string | null = null;
  let alive = true;
  let epoch = 0;
  let captureSeeded = false;
  let loadingEarlier = false;
  let feedChain: Promise<void> = Promise.resolve();

  const loadEarlier = nonActivatingButton("Load earlier messages", LANDMARKS.widgetFeedLoadEarlier, () => {
    afterPointer(() => {
      if (atHistoryCapacity()) {
        const target = oldestTranscriptEventKey(state.exchanges);
        if (target) {
          void options.api.openWidgetEvent(target);
        }
        return;
      }
      void enqueue(loadEarlierPage);
    });
  });
  loadEarlier.classList.add("widget-feed-load-earlier", "widget-feed-target");

  const scrollport = el("div", { id: LANDMARKS.widgetFeedScroll, className: "widget-feed-scroll" });
  const list = el("div", { id: LANDMARKS.widgetFeedList, className: "widget-feed-list" });
  const idle = el("p", { className: "widget-feed-idle" });
  scrollport.append(list);

  const liveButton = nonActivatingButton("Following live", LANDMARKS.widgetFeedLiveToggle, () => {
    afterPointer(() => {
      void enqueue(reloadNewest);
    });
  });
  liveButton.classList.add("widget-feed-target");
  liveButton.setAttribute("aria-pressed", "true");

  const jumpButton = nonActivatingButton(HISTORY_LABEL, LANDMARKS.widgetFeedJumpLive, () => {
    afterPointer(() => {
      void enqueue(reloadNewest);
    });
  });
  jumpButton.classList.add("widget-feed-target");
  jumpButton.hidden = true;

  const openButton = nonActivatingButton("Open transcript", LANDMARKS.widgetFeedOpenTranscript, () => {
    afterPointer(() => {
      const target = transcriptEventKey(state.exchanges);
      if (!target) {
        return;
      }
      void options.api.openWidgetEvent(target);
    });
  });
  openButton.classList.add("widget-feed-target");

  const controls = el("div", {
    id: LANDMARKS.widgetSurfaceControls,
    className: "widget-surface-controls",
    attrs: { role: "group", "aria-label": "Conversation" },
    children: [liveButton, jumpButton, openButton],
  });
  const banner = el("p", { className: "widget-feed-banner" });
  banner.hidden = true;
  const mode = el("p", { className: "widget-feed-mode" });
  const statusRegion = el("div", {
    id: LANDMARKS.widgetFeedStatus,
    className: "widget-feed-status",
    children: [banner, mode],
  });
  const column = el("div", {
    className: "widget-feed-column",
    children: [loadEarlier, scrollport, controls, statusRegion],
  });
  options.parent.replaceChildren(column);

  const onScroll = (): void => {
    if (
      !userScrollShouldPause({
        followingLive: state.followingLive,
        programmatic: consumeProgrammaticScroll(scrollport),
        distanceFromBottomPx: distanceFromBottom(scrollport),
      })
    ) {
      return;
    }
    state.followingLive = false;
    paint("none");
  };
  scrollport.addEventListener("scroll", onScroll);

  const enqueue = (task: () => Promise<void>): Promise<void> => {
    const run = feedChain.then(task, task);
    feedChain = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  };

  const poller = createSingleFlightPoller(async () => {
    await enqueue(pollOnce);
  }, WIDGET_POLL_MS);

  function afterPointer(action: () => void): void {
    const reassertion = options.takePointerReassertion();
    void Promise.resolve(reassertion)
      .then(() => {
        if (!alive) {
          return;
        }
        action();
      })
      .catch(() => undefined);
  }

  async function pollOnce(): Promise<void> {
    const token = epoch;
    const revisionBefore = paintedRevision;
    try {
      const [nextStatus, page] = await Promise.all([
        options.api.getStatus(),
        options.api.getWidgetFeed(null),
      ]);
      if (!alive || token !== epoch) {
        return;
      }
      state.status = nextStatus;
      state.loadError = null;
      const first = state.historyToken === null;
      const applied = acceptNewest(page);
      if (!alive || token !== epoch) {
        return;
      }
      if (applied === "invalid") {
        state.loadError = FEED_LOAD_ERROR;
        paint("none");
      } else if (applied === "applied" && first && !captureSeeded && capturePresentation === "paused-unread") {
        captureSeeded = true;
        state.followingLive = false;
        state.unread = unique(collectEventKeys(state.exchanges));
        paint("preserve");
      } else if (applied === "applied" && first && !captureSeeded && capturePresentation === "expanded") {
        captureSeeded = true;
        paint("pin");
        await expandCapturedMessages();
      } else if (applied === "applied") {
        paint(state.followingLive ? "pin-if-moved" : "preserve");
      } else {
        paint("none");
      }
    } catch {
      if (!alive || token !== epoch) {
        return;
      }
      state.loadError = FEED_LOAD_ERROR;
      paint("none");
    }
    if (alive && token === epoch) {
      options.onPolled(paintedRevision !== revisionBefore);
    }
  }

  async function loadEarlierPage(): Promise<void> {
    if (!alive || loadingEarlier || state.historyChanged || !state.hasEarlier) {
      return;
    }
    const anchor = state.nextBeforeExchangeKey ?? state.exchanges[0]?.exchangeKey ?? null;
    if (!anchor) {
      return;
    }
    const token = epoch;
    loadingEarlier = true;
    paint("none");
    try {
      const page = await options.api.getWidgetFeed(anchor);
      if (!alive || token !== epoch) {
        return;
      }
      state.loadError = null;
      const applied = acceptEarlier(page);
      if (applied === "invalid") {
        state.loadError = FEED_LOAD_ERROR;
        paint("none");
        return;
      }
      paint(applied === "applied" ? "anchor" : "none");
    } catch {
      if (!alive || token !== epoch) {
        return;
      }
      state.loadError = FEED_LOAD_ERROR;
      paint("none");
    } finally {
      loadingEarlier = false;
      if (alive && token === epoch) {
        paint("none");
      }
    }
  }

  async function reloadNewest(): Promise<void> {
    const token = epoch;
    try {
      const page = await options.api.getWidgetFeed(null);
      if (!alive || token !== epoch) {
        return;
      }
      if (!replaceNewest(page)) {
        state.loadError = FEED_LOAD_ERROR;
        paint("none");
        return;
      }
      state.loadError = null;
      paint("pin");
    } catch {
      if (!alive || token !== epoch) {
        return;
      }
      state.loadError = FEED_LOAD_ERROR;
      paint("none");
    }
  }

  async function expandCapturedMessages(): Promise<void> {
    for (const key of collectEventKeys(state.exchanges)) {
      const message = findMessage(key);
      if (message?.truncated) {
        await showFull(key, true);
      }
    }
  }

  async function showFull(eventKey: string, force = false): Promise<void> {
    const existing = findMessage(eventKey);
    if (!existing || expanding.has(eventKey)) {
      return;
    }
    if (!existing.truncated && !existing.expanded && !force) {
      return;
    }
    if (existing.expanded && !force) {
      existing.expanded = false;
      paint(state.followingLive ? "pin-if-moved" : "preserve");
      return;
    }
    expanding.add(eventKey);
    paint("none");
    try {
      const loaded = await options.api.getWidgetMessage(eventKey);
      if (!alive) {
        return;
      }
      const current = findMessage(eventKey);
      if (!current) {
        return;
      }
      if (
        !loaded ||
        loaded.eventKey !== eventKey ||
        loaded.projection === "withheld" ||
        typeof loaded.body !== "string"
      ) {
        current.expansionError = FULL_MESSAGE_ERROR;
        current.expanded = false;
        paint(state.followingLive ? "pin-if-moved" : "preserve");
        return;
      }
      current.fullBody = loaded.body;
      current.expanded = true;
      current.expansionError = null;
      paint(state.followingLive ? "pin-if-moved" : "preserve");
    } catch {
      if (!alive) {
        return;
      }
      const current = findMessage(eventKey);
      if (current) {
        current.expansionError = FULL_MESSAGE_ERROR;
        current.expanded = false;
      }
      paint(state.followingLive ? "pin-if-moved" : "preserve");
    } finally {
      expanding.delete(eventKey);
      if (alive) {
        paint("none");
      }
    }
  }

  function paint(hold: ScrollHold): boolean {
    const nextList = `${listSignature(state.exchanges)}\u001e${[...expanding].sort().join(",")}`;
    const listChanged = nextList !== paintedList;
    const beforeTop = scrollport.scrollTop;
    const beforeHeight = scrollport.scrollHeight;
    if (listChanged) {
      paintedList = nextList;
      reconcileList();
    }
    paintChrome();
    if (hold === "pin" || (hold === "pin-if-moved" && listChanged)) {
      assignProgrammaticScrollTop(scrollport, scrollport.scrollHeight);
    } else if (hold === "anchor" && listChanged) {
      assignProgrammaticScrollTop(scrollport, beforeTop + (scrollport.scrollHeight - beforeHeight));
    } else if (hold === "preserve" && listChanged) {
      assignProgrammaticScrollTop(scrollport, beforeTop);
    }
    const revision = `${nextList}\u001d${chromeSignature()}`;
    const changed = revision !== paintedRevision;
    if (changed) {
      paintedRevision = revision;
      options.onPresented(true);
    }
    return changed;
  }

  function reconcileList(): void {
    if (state.exchanges.length === 0) {
      setText(idle, idleCopy());
      idle.hidden = idle.textContent?.length === 0;
      list.replaceChildren(idle);
      return;
    }
    const nodes: HTMLElement[] = [];
    const exchangeKeep = new Set<string>();
    const rowKeep = new Set<string>();
    const dividerKeep = new Set<string>();
    let previousSession: string | null = null;
    for (const exchange of state.exchanges) {
      if (previousSession !== null && exchange.sessionKey !== previousSession) {
        const dividerKey = `divider:${exchange.exchangeKey}`;
        nodes.push(sessionDivider(dividerKey));
        dividerKeep.add(dividerKey);
      }
      previousSession = exchange.sessionKey;
      nodes.push(syncExchange(exchange, rowKeep));
      exchangeKeep.add(exchange.exchangeKey);
    }
    list.replaceChildren(...nodes);
    for (const key of exchangeNodes.keys()) {
      if (!exchangeKeep.has(key)) {
        exchangeNodes.delete(key);
      }
    }
    for (const key of rowNodes.keys()) {
      if (!rowKeep.has(key)) {
        rowNodes.delete(key);
      }
    }
    for (const key of dividerNodes.keys()) {
      if (!dividerKeep.has(key)) {
        dividerNodes.delete(key);
      }
    }
  }

  function syncExchange(exchange: StoredExchange, rowKeep: Set<string>): HTMLElement {
    let article = exchangeNodes.get(exchange.exchangeKey);
    if (!article) {
      article = el("article", { className: "widget-feed-exchange" });
      exchangeNodes.set(exchange.exchangeKey, article);
    }
    article.dataset.exchangeKey = exchange.exchangeKey;
    const rows = rowModels(exchange);
    const children: HTMLElement[] = [];
    for (const model of rows) {
      let handle = rowNodes.get(model.key);
      if (!handle) {
        handle = createRow();
        rowNodes.set(model.key, handle);
      }
      fillRow(handle, model);
      children.push(handle.root);
      rowKeep.add(model.key);
    }
    const same =
      article.childElementCount === children.length &&
      children.every((node, index) => article?.children.item(index) === node);
    if (!same) {
      article.replaceChildren(...children);
    }
    return article;
  }

  function createRow(): RowHandle {
    const root = el("div", { className: "widget-feed-row" });
    const name = el("p", { className: "widget-feed-name" });
    const meta = el("p", { className: "widget-feed-meta" });
    const body = el("p", { className: "widget-feed-body" });
    const note = el("p", { className: "widget-feed-note" });
    const error = el("p", { className: "widget-feed-error" });
    const fullButton = nonActivatingButton("Show full message", "", () => {
      const key = root.dataset.eventKey;
      if (key) {
        afterPointer(() => {
          void showFull(key);
        });
      }
    });
    fullButton.classList.add("widget-feed-target", "widget-feed-full");
    const openMessage = nonActivatingButton("Open in transcript", "", () => {
      const key = root.dataset.eventKey;
      if (!key) {
        return;
      }
      afterPointer(() => {
        void options.api.openWidgetEvent(key);
      });
    });
    openMessage.classList.add("widget-feed-target", "widget-feed-open-message");
    const slip = el("div", {
      className: "widget-feed-slip",
      children: [name, meta, body, note, error, fullButton, openMessage],
    });
    root.append(slip);
    return {
      root,
      name,
      meta,
      body,
      note,
      error,
      fullButton,
      openButton: openMessage,
      avatar: null,
    };
  }

  function fillRow(handle: RowHandle, model: RowModel): void {
    handle.root.dataset.kind = model.kind;
    handle.root.dataset.speaker = model.speaker;
    handle.root.dataset.tail = model.tail;
    handle.root.dataset.projection = model.projection;
    if (model.eventKey) {
      handle.root.dataset.eventKey = model.eventKey;
    } else {
      delete handle.root.dataset.eventKey;
    }
    syncAvatar(handle, model.avatar);
    setText(handle.name, model.displayName);
    setText(handle.meta, model.meta);
    handle.body.hidden = model.projection === "withheld";
    setText(handle.body, model.projection === "withheld" ? "" : model.body);
    handle.note.hidden = model.note == null;
    setText(handle.note, model.note ?? "");
    handle.error.hidden = model.error == null;
    setText(handle.error, model.error ?? "");
    handle.fullButton.hidden = !model.showFull;
    handle.fullButton.disabled = model.eventKey != null && expanding.has(model.eventKey);
    setText(handle.fullButton, model.expanded ? "Collapse" : "Show full message");
    handle.openButton.hidden = model.eventKey == null;
  }

  function syncAvatar(handle: RowHandle, avatar: AvatarKind): void {
    const current = handle.avatar?.dataset.avatarKind;
    if (current === avatar) {
      return;
    }
    handle.avatar?.remove();
    handle.avatar = null;
    if (avatar === "none") {
      return;
    }
    const node = avatar === "neutral" ? neutralDevice() : idleAvatar(avatar);
    node.dataset.avatarKind = avatar;
    handle.root.prepend(node);
    handle.avatar = node;
  }

  function paintChrome(): void {
    const following = state.followingLive;
    liveButton.setAttribute("aria-pressed", following ? "true" : "false");
    liveButton.classList.toggle("is-latched", following);
    const jumpText = jumpCopy();
    jumpButton.hidden = jumpText == null;
    setText(jumpButton, jumpText ?? "");
    openButton.disabled = transcriptEventKey(state.exchanges) == null;
    const earlierAnchor = state.nextBeforeExchangeKey ?? state.exchanges[0]?.exchangeKey ?? null;
    const historyCapacity = atHistoryCapacity();
    setText(loadEarlier, historyCapacity ? "Open earlier messages in transcript" : "Load earlier messages");
    loadEarlier.disabled =
      loadingEarlier ||
      state.historyChanged ||
      !state.hasEarlier ||
      (historyCapacity ? oldestTranscriptEventKey(state.exchanges) == null : earlierAnchor == null);
    const bannerText = degradedBanner(state.status);
    banner.hidden = bannerText == null;
    setText(banner, bannerText ?? "");
    setText(mode, modeCopy());
    if (state.exchanges.length === 0 && list.firstElementChild === idle) {
      setText(idle, idleCopy());
      idle.hidden = idle.textContent?.length === 0;
    }
    const interactive = state.status?.desktopRuntimeState === "interactive";
    if (interactive) {
      statusRegion.setAttribute("role", "status");
      statusRegion.setAttribute("aria-live", "polite");
      statusRegion.setAttribute("aria-atomic", "true");
    } else {
      statusRegion.setAttribute("aria-live", "off");
      statusRegion.removeAttribute("role");
      statusRegion.removeAttribute("aria-atomic");
    }
  }

  function modeCopy(): string {
    if (state.historyChanged) {
      return HISTORY_LABEL;
    }
    if (state.loadError) {
      return state.loadError;
    }
    if (!state.followingLive && state.unread.length > 0) {
      return `Jump to live \u00b7 ${state.unread.length} new`;
    }
    if (!state.followingLive) {
      return "Not following live";
    }
    return "Following live";
  }

  function jumpCopy(): string | null {
    if (state.historyChanged) {
      return HISTORY_LABEL;
    }
    if (!state.followingLive && state.unread.length > 0) {
      return `Jump to live \u00b7 ${state.unread.length} new`;
    }
    return null;
  }

  function idleCopy(): string {
    if (state.loadError) {
      return "";
    }
    if (!state.status) {
      return "Connecting\u2026";
    }
    return idleWidgetLabel(state.status.sourceState);
  }

  function chromeSignature(): string {
    return [
      modeCopy(),
      degradedBanner(state.status) ?? "",
      state.followingLive ? "live" : "paused",
      String(state.unread.length),
      state.historyChanged ? "reset" : "same",
      loadEarlier.disabled ? "earlier-off" : "earlier-on",
      atHistoryCapacity() ? "earlier-transcript" : "earlier-feed",
      openButton.disabled ? "open-off" : "open-on",
      state.status?.desktopRuntimeState ?? "",
    ].join("\u001f");
  }

  function sessionDivider(key: string): HTMLParagraphElement {
    const existing = dividerNodes.get(key);
    if (existing) {
      return existing;
    }
    const node = el("p", { className: "widget-feed-divider", text: "New conversation" });
    dividerNodes.set(key, node);
    return node;
  }

  function findMessage(eventKey: string): StoredMessage | null {
    for (const exchange of state.exchanges) {
      if (exchange.request?.eventKey === eventKey) {
        return exchange.request;
      }
      if (exchange.completion?.eventKey === eventKey) {
        return exchange.completion;
      }
    }
    return null;
  }

  function acceptNewest(page: WidgetFeedPage): "applied" | "rejected" | "invalid" {
    if (!isFeedPage(page)) {
      return "invalid";
    }
    const established = state.historyToken !== null;
    if (established && page.resetRequired) {
      state.historyChanged = true;
      state.followingLive = false;
      return "rejected";
    }
    if (established && page.historyToken !== state.historyToken) {
      if (!state.followingLive) {
        state.historyChanged = true;
        return "rejected";
      }
      return replaceNewest(page) ? "applied" : "invalid";
    }
    const incoming = dedupeExchanges(clampFeedPage(page.items));
    if (!established) {
      state.historyToken = page.historyToken;
      state.exchanges = trimExchanges(incoming.map((item) => toExchange(item)));
      state.hasEarlier = page.hasEarlier === true;
      state.nextBeforeExchangeKey = page.nextBeforeExchangeKey;
      return "applied";
    }
    const incomingKeys = new Set(incoming.map((item) => item.exchangeKey));
    let overlap = -1;
    for (let index = 0; index < state.exchanges.length; index += 1) {
      const exchange = state.exchanges[index];
      if (exchange && incomingKeys.has(exchange.exchangeKey)) {
        overlap = index;
        break;
      }
    }
    if (state.exchanges.length > 0 && overlap < 0) {
      state.historyChanged = true;
      state.followingLive = false;
      return "rejected";
    }
    const previousKeys = new Set(collectEventKeys(state.exchanges));
    const prefix = overlap < 0 ? [] : state.exchanges.slice(0, overlap);
    const previousByKey = new Map(state.exchanges.map((exchange) => [exchange.exchangeKey, exchange]));
    const tail = incoming.map((item) => toExchange(item, previousByKey.get(item.exchangeKey)));
    const merged = trimExchanges(prefix.concat(tail));
    if (state.followingLive) {
      state.unread = [];
    } else {
      rememberUnread(previousKeys, merged);
    }
    state.exchanges = merged;
    retainVisibleUnread();
    state.historyToken = page.historyToken;
    if (prefix.length === 0) {
      state.hasEarlier = page.hasEarlier === true;
      state.nextBeforeExchangeKey = page.nextBeforeExchangeKey;
    }
    return "applied";
  }

  function acceptEarlier(page: WidgetFeedPage): "applied" | "rejected" | "invalid" {
    if (!isFeedPage(page)) {
      return "invalid";
    }
    if (state.historyToken === null || page.resetRequired || page.historyToken !== state.historyToken) {
      state.historyChanged = true;
      state.followingLive = false;
      return "rejected";
    }
    const existing = new Set(state.exchanges.map((exchange) => exchange.exchangeKey));
    const older = dedupeExchanges(clampFeedPage(page.items))
      .filter((item) => !existing.has(item.exchangeKey))
      .map((item) => toExchange(item));
    const capacity = Math.max(0, FEED_MAX_EXCHANGES - state.exchanges.length);
    const retainedOlder = older.slice(Math.max(0, older.length - capacity));
    state.exchanges = retainedOlder.concat(state.exchanges);
    retainVisibleUnread();
    state.followingLive = false;
    state.hasEarlier = page.hasEarlier === true || retainedOlder.length < older.length;
    state.nextBeforeExchangeKey = retainedOlder[0]?.exchangeKey ?? page.nextBeforeExchangeKey;
    return "applied";
  }

  function atHistoryCapacity(): boolean {
    return state.hasEarlier && state.exchanges.length >= FEED_MAX_EXCHANGES;
  }

  function replaceNewest(page: WidgetFeedPage): boolean {
    if (!isFeedPage(page)) {
      return false;
    }
    const previousByKey = new Map(state.exchanges.map((exchange) => [exchange.exchangeKey, exchange]));
    state.historyToken = page.historyToken;
    state.historyChanged = false;
    state.followingLive = true;
    state.unread = [];
    state.exchanges = trimExchanges(
      dedupeExchanges(clampFeedPage(page.items)).map((item) => toExchange(item, previousByKey.get(item.exchangeKey))),
    );
    state.hasEarlier = page.hasEarlier === true;
    state.nextBeforeExchangeKey = page.nextBeforeExchangeKey;
    return true;
  }

  function rememberUnread(previous: Set<string>, exchanges: readonly StoredExchange[]): void {
    const seen = new Set(state.unread);
    for (const key of collectEventKeys(exchanges)) {
      if (!previous.has(key) && !seen.has(key)) {
        seen.add(key);
        state.unread.push(key);
      }
    }
  }

  function retainVisibleUnread(): void {
    const present = new Set(collectEventKeys(state.exchanges));
    state.unread = state.unread.filter((key) => present.has(key));
  }

  paint("none");
  poller.start();

  return {
    stop() {
      alive = false;
      epoch += 1;
      poller.stop();
      scrollport.removeEventListener("scroll", onScroll);
    },
  };
}

function readCapturePresentation(): "paused-unread" | "expanded" | null {
  const values = [fixtureParam(), document.documentElement.dataset.syntheticFixture ?? ""];
  for (const value of values) {
    if (value === "paused-unread") {
      return "paused-unread";
    }
    if (value === "expanded") {
      return "expanded";
    }
  }
  return null;
}

function fixtureParam(): string {
  try {
    return new URLSearchParams(window.location.search).get("fixture") ?? "";
  } catch {
    return "";
  }
}

export function clampFeedPage<T>(items: readonly T[]): T[] {
  if (items.length <= FEED_PAGE_EXCHANGES) {
    return [...items];
  }
  return items.slice(items.length - FEED_PAGE_EXCHANGES);
}

export function trimExchanges<T>(items: readonly T[]): T[] {
  if (items.length <= FEED_MAX_EXCHANGES) {
    return [...items];
  }
  return items.slice(items.length - FEED_MAX_EXCHANGES);
}

function dedupeExchanges(items: readonly WidgetFeedExchange[]): WidgetFeedExchange[] {
  const seen = new Set<string>();
  const result: WidgetFeedExchange[] = [];
  for (const item of items) {
    if (!item || typeof item.exchangeKey !== "string" || seen.has(item.exchangeKey)) {
      continue;
    }
    seen.add(item.exchangeKey);
    result.push(item);
  }
  return result;
}

function isFeedPage(page: WidgetFeedPage | null | undefined): page is WidgetFeedPage {
  return Boolean(page && typeof page.historyToken === "string" && Array.isArray(page.items));
}

function oldestTranscriptEventKey(exchanges: readonly StoredExchange[]): string | null {
  const oldest = exchanges[0];
  return oldest?.request?.eventKey ?? oldest?.completion?.eventKey ?? null;
}

function toExchange(exchange: WidgetFeedExchange, previous?: StoredExchange): StoredExchange {
  const request = exchange.request
    ? toMessage(exchange.request, previous?.request?.eventKey === exchange.request.eventKey ? previous.request : null)
    : null;
  const completion = exchange.completion
    ? toMessage(
        exchange.completion,
        previous?.completion?.eventKey === exchange.completion.eventKey ? previous.completion : null,
      )
    : null;
  return {
    exchangeKey: exchange.exchangeKey,
    sessionKey: exchange.sessionKey,
    timestampMs: exchange.timestampMs,
    request,
    completion,
    pendingLabel: completion ? null : exchange.pendingLabel,
  };
}

function toMessage(message: WidgetFeedMessage, previous: StoredMessage | null): StoredMessage {
  return {
    eventKey: message.eventKey,
    eventType: message.eventType,
    speaker: message.speaker,
    recipient: message.recipient,
    timestampMs: message.timestampMs,
    status: message.status,
    automaticBody: message.body,
    fullBody: previous?.fullBody ?? null,
    expanded: previous?.expanded ?? false,
    truncated: message.truncated === true,
    fullCharacterLength: message.fullCharacterLength,
    projection: message.projection,
    contextOmitted: message.contextOmitted === true,
    expansionError: previous?.expansionError ?? null,
  };
}

function collectEventKeys(exchanges: readonly StoredExchange[]): string[] {
  const keys: string[] = [];
  for (const exchange of exchanges) {
    if (exchange.request) {
      keys.push(exchange.request.eventKey);
    }
    if (exchange.completion) {
      keys.push(exchange.completion.eventKey);
    }
  }
  return keys;
}

function unique(keys: readonly string[]): string[] {
  return [...new Set(keys)];
}

export function transcriptEventKey(
  exchanges: readonly { request: { eventKey: string } | null; completion: { eventKey: string } | null }[],
): string | null {
  const newest = exchanges[exchanges.length - 1];
  if (!newest) {
    return null;
  }
  if (newest.completion) {
    return newest.completion.eventKey;
  }
  if (newest.request) {
    return newest.request.eventKey;
  }
  return null;
}

function rowModels(exchange: StoredExchange): RowModel[] {
  const rows: RowModel[] = [];
  if (exchange.request) {
    rows.push(messageModel(exchange.request, "request"));
  }
  if (exchange.completion) {
    rows.push(messageModel(exchange.completion, "completion"));
  } else if (exchange.pendingLabel) {
    rows.push(pendingModel(exchange));
  }
  return rows;
}

function messageModel(message: StoredMessage, role: "request" | "completion"): RowModel {
  if (message.eventType === "error") {
    return {
      key: message.eventKey,
      eventKey: message.eventKey,
      kind: "parley-error",
      speaker: "unknown",
      displayName: "Parley",
      tail: "none",
      avatar: "none",
      meta: `${PARLEY_ERROR_LABEL} \u00b7 ${formatTimestamp(message.timestampMs)}`,
      body: visibleBody(message),
      note: visibleNote(message),
      error: message.expansionError,
      showFull: canExpand(message),
      expanded: message.expanded,
      projection: message.projection,
    };
  }
  const known = normalizeSpeaker(message.speaker);
  return {
    key: message.eventKey,
    eventKey: message.eventKey,
    kind: "speech",
    speaker: known ?? "unknown",
    displayName: displaySpeaker(message.speaker),
    tail: tailForSpeaker(message.speaker),
    avatar: known ?? "neutral",
    meta: `${role === "request" ? "Request" : "Completion"} \u00b7 ${formatTimestamp(message.timestampMs)}`,
    body: visibleBody(message),
    note: visibleNote(message),
    error: message.expansionError,
    showFull: canExpand(message),
    expanded: message.expanded,
    projection: message.projection,
  };
}

function pendingModel(exchange: StoredExchange): RowModel {
  const recipient = exchange.request?.recipient ?? "";
  const known = normalizeSpeaker(recipient);
  return {
    key: `pending:${exchange.exchangeKey}`,
    eventKey: null,
    kind: "pending",
    speaker: known ?? "unknown",
    displayName: known ? displaySpeaker(recipient) : "Parley",
    tail: known ? tailForSpeaker(recipient) : "none",
    avatar: known ?? "none",
    meta: "Pending response",
    body: exchange.pendingLabel ?? "",
    note: null,
    error: null,
    showFull: false,
    expanded: false,
    projection: "exact",
  };
}

function visibleBody(message: StoredMessage): string {
  if (message.projection === "withheld") {
    return "";
  }
  if (message.expanded && message.fullBody != null) {
    return message.fullBody;
  }
  return message.automaticBody;
}

function visibleNote(message: StoredMessage): string | null {
  if (message.projection === "withheld") {
    return "Message withheld";
  }
  if (message.contextOmitted) {
    return "Earlier context omitted";
  }
  return null;
}

function canExpand(message: StoredMessage): boolean {
  return message.projection !== "withheld" && (message.truncated || message.expanded);
}

function listSignature(exchanges: readonly StoredExchange[]): string {
  return exchanges
    .map((exchange) =>
      [
        exchange.exchangeKey,
        exchange.sessionKey,
        exchange.pendingLabel ?? "",
        messageSignature(exchange.request),
        messageSignature(exchange.completion),
      ].join("\u001f"),
    )
    .join("\u001e");
}

function messageSignature(message: StoredMessage | null): string {
  if (!message) {
    return "";
  }
  return [
    message.eventKey,
    message.eventType,
    message.speaker,
    message.automaticBody,
    message.fullBody ?? "",
    message.expanded ? "1" : "0",
    message.expansionError ?? "",
    message.truncated ? "1" : "0",
    message.projection,
    message.contextOmitted ? "1" : "0",
  ].join("\u001d");
}

function idleAvatar(agent: "codex" | "grok"): HTMLImageElement {
  const source = agent === "codex" ? AVATAR_ILLUSTRATIONS.codexIdle : AVATAR_ILLUSTRATIONS.grokIdle;
  const image = el("img", {
    className: "widget-feed-avatar",
    attrs: {
      src: source.path,
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

function neutralDevice(): HTMLSpanElement {
  const screen = el("span", { className: "widget-feed-device-screen" });
  return el("span", {
    className: "widget-feed-device",
    attrs: { "aria-hidden": "true" },
    children: [screen],
  });
}

function nonActivatingButton(label: string, id: string, onClick: () => void): HTMLButtonElement {
  const control = el("button", {
    ...(id ? { id } : {}),
    className: "widget-surface-button",
    text: label,
    attrs: { type: "button", tabindex: "-1" },
  });
  control.tabIndex = -1;
  control.addEventListener("click", () => {
    if (control.disabled) {
      return;
    }
    onClick();
  });
  control.addEventListener("mousedown", (event) => {
    event.preventDefault();
  });
  control.addEventListener("keydown", (event) => {
    event.preventDefault();
    event.stopPropagation();
  });
  control.addEventListener("focus", () => {
    control.blur();
  });
  return control;
}
