import type {
  EventContent,
  ExchangeSummary,
  HandoffSelection,
  MonitorInfo,
  PeerActivitySnapshot,
  PeerHealthSnapshot,
  SearchHit,
  SessionSummary,
  SourceStatus,
  ViewerSettings,
  ViewerStatus,
} from "../contracts";
import { DEFAULT_SETTINGS } from "../contracts";
import type { ViewerApi } from "../ipc";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { el, labelledControl, setText } from "./dom";
import { exactEventBody } from "./excerpt";
import {
  formatCharacterCount,
  formatDiagnostics,
  formatDuration,
  formatEventType,
  formatRoute,
  formatRuntimeHealth,
  formatSourceLine,
  formatSourceState,
  formatTimestamp,
} from "./format";
import {
  activityAttentionLabel,
  buildInspector,
  pendingHandoffCount,
  unreadIncidentCount,
} from "./inspector-tabs";
import { degradedBanner, loadErrorLabel } from "./labels";
import { LANDMARKS } from "./landmarks";
import {
  applyPageResult,
  canGoNext,
  canGoPrevious,
  dataRefreshPlan,
  pageRangeLabel,
  requestNextPage,
  requestPreviousPage,
  resetPaging,
  type PagingState,
} from "./paging";
import {
  applyOpenHandoff,
  buildPeerHealthSection,
  paintPeerHealth as paintPeerHealthView,
  presentPeerHealth,
  TEST_CHIME_REQUESTED_LABEL,
} from "./peer-health";
import {
  buildPeerActivitySection,
  paintPeerActivity as paintPeerActivityView,
  peerActivityRevision,
  presentPeerActivity,
} from "./peer-activity";
import { createSingleFlightPoller, DETAIL_POLL_MS } from "./poll";
import {
  createSearchState,
  isSearchActive,
  searchCanPage,
  searchSummary,
  submitSearch,
  type SearchViewState,
} from "./search";
import { isFocusLocked } from "./timeline";
import {
  buildSessionRail,
  buildTimeline,
  paintExchangeTimeline,
  paintSearchTimeline,
  paintSessionRail,
} from "./timeline";

const CORNERS: Array<ViewerSettings["corner"]> = [
  "top-left",
  "top-right",
  "bottom-left",
  "bottom-right",
];

interface DetailState {
  status: ViewerStatus | null;
  settings: ViewerSettings;
  monitors: MonitorInfo[];
  sessions: SessionSummary[];
  sessionPaging: PagingState;
  selectedSessionKey: string | null;
  exchanges: ExchangeSummary[];
  exchangePaging: PagingState;
  selectedExchangeKey: string | null;
  selectedEventKey: string | null;
  search: SearchViewState;
  searchHits: SearchHit[];
  event: EventContent | null;
  eventLoading: boolean;
  eventError: string | null;
  loadError: string | null;
  selectingLog: boolean;
  savingSettings: boolean;
  settingsDirty: boolean;
  exiting: boolean;
  peerHealth: PeerHealthSnapshot | null;
  peerHealthError: string | null;
  peerHealthLoading: boolean;
  peerHealthBusy: boolean;
  peerActivity: PeerActivitySnapshot | null;
  peerActivityError: string | null;
  peerActivityLoading: boolean;
  peerActivityRevision: string | null;
  handoffLabel: string | null;
  chimeStatus: string | null;
}

export function mountDetail(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  root.className = "detail-shell";
  root.removeAttribute("aria-live");

  const sessions = buildSessionRail();
  const timeline = buildTimeline();
  const inspector = buildInspector();
  const desktopMode = el("select", {
    attrs: { id: "desktop-mode", "aria-label": "Desktop interaction mode" },
    children: [
      el("option", { text: "Interactive conversation", attrs: { value: "interactive" } }),
      el("option", { text: "Passive scene", attrs: { value: "passive" } }),
    ],
  });
  const retryInteractive = el("button", {
    className: "studio-action",
    text: "Retry interactive mode",
    attrs: { type: "button" },
  });
  const desktopRuntime = el("p", {
    className: "studio-diagnostics",
    attrs: { "aria-live": "polite" },
  });
  inspector.settingsForm.insertBefore(
    labelledControl("Desktop mode", desktopMode),
    inspector.saveSettings,
  );
  inspector.panels.settings.append(
    el("div", {
      className: "studio-settings-runtime",
      children: [desktopRuntime, retryInteractive],
    }),
  );
  const peerActivity = buildPeerActivitySection();
  const peerHealth = buildPeerHealthSection();
  inspector.activityHost.append(peerActivity.section, peerHealth.section);

  const sourceLine = el("p", { className: "studio-status", id: "source-status" });
  const healthLine = el("p", { className: "studio-health" });
  const exit = el("button", {
    className: "studio-action studio-exit",
    text: "Exit",
    attrs: {
      type: "button",
      "data-region": LANDMARKS.studioExit,
      "aria-label": "Exit Parley Conversation Viewer",
    },
  });
  const logo = el("img", {
    className: "studio-logo",
    attrs: { src: "/parley-icon.ico", alt: "", width: "28", height: "28" },
  });
  logo.setAttribute("aria-hidden", "true");
  const title = el("h1", { className: "studio-title", text: "Conversation studio" });
  const banner = el("p", {
    className: "studio-banner",
    attrs: { role: "alert", "data-region": LANDMARKS.studioBanner },
  });
  banner.hidden = true;
  const loadError = el("p", { className: "studio-load-error", attrs: { role: "status" } });
  loadError.hidden = true;
  const shell = el("div", {
    className: "studio",
    children: [
      el("header", {
        className: "studio-bar",
        children: [
          el("div", { className: "studio-brand", children: [logo, title] }),
          el("div", { className: "studio-bar-copy", children: [sourceLine, healthLine] }),
          exit,
        ],
      }),
      banner,
      loadError,
      el("div", {
        className: "studio-body",
        children: [sessions.region, timeline.region, inspector.region],
      }),
    ],
  });
  root.replaceChildren(shell);

  const state: DetailState = {
    status: null,
    settings: { ...DEFAULT_SETTINGS },
    monitors: [],
    sessions: [],
    sessionPaging: resetPaging(),
    selectedSessionKey: null,
    exchanges: [],
    exchangePaging: resetPaging(),
    selectedExchangeKey: null,
    selectedEventKey: null,
    search: createSearchState(),
    searchHits: [],
    event: null,
    eventLoading: false,
    eventError: null,
    loadError: null,
    selectingLog: false,
    savingSettings: false,
    settingsDirty: false,
    exiting: false,
    peerHealth: null,
    peerHealthError: null,
    peerHealthLoading: true,
    peerActivity: null,
    peerActivityError: null,
    peerActivityLoading: true,
    peerActivityRevision: null,
    peerHealthBusy: false,
    handoffLabel: null,
    chimeStatus: null,
  };

  let alive = true;
  let sourceEpoch = 0;
  let sessionLoad = 0;
  let exchangeLoad = 0;
  let searchLoad = 0;
  let healthLoad = 0;
  let activityLoad = 0;

  const paintAttention = (): void => {
    inspector.setActivityAttention(
      activityAttentionLabel(
        unreadIncidentCount(state.peerHealth),
        pendingHandoffCount(state.peerActivity),
      ),
    );
  };

  const paintChrome = (): void => {
    const bannerText = degradedBanner(state.status);
    banner.hidden = bannerText == null;
    setText(banner, bannerText ?? "");
    setText(sourceLine, state.status ? formatSourceLine(state.status) : "Connecting\u2026");
    setText(
      healthLine,
      state.status ? formatRuntimeHealth(state.status) : "Tray unknown. Underlay unknown",
    );
    setText(
      inspector.diagnostics,
      state.status
        ? `${formatDiagnostics(state.status.diagnostics)}. Generation ${Math.trunc(state.status.generation)}; ${Math.trunc(state.status.bytesRead)} bytes`
        : "No diagnostics yet",
    );
    if (state.loadError) {
      loadError.hidden = false;
      setText(loadError, state.loadError);
    } else {
      loadError.hidden = true;
      setText(loadError, "");
    }
    inspector.widgetVisible.checked = state.status?.widgetVisible === true;
    inspector.launchAtLogin.checked = state.settings.launchAtLogin;
    inspector.selectLog.disabled = state.selectingLog;
    renderSourceList(
      inspector.sourceList,
      inspector.sourceEmpty,
      state.status?.sources ?? [],
      state.selectingLog,
      (path) => void removeSource(path),
    );
    inspector.saveSettings.disabled = state.savingSettings;
    retryInteractive.disabled =
      state.savingSettings ||
      state.settings.desktopMode !== "interactive" ||
      state.status?.desktopRuntimeState !== "passive-fallback";
    setText(desktopRuntime, desktopRuntimeLabel(state.status));
    exit.disabled = state.exiting;
    if (!state.settingsDirty) {
      syncSettingsForm();
    }
  };

  const paintSessions = (): void => {
    setText(sessions.meta, pageRangeLabel(state.sessionPaging));
    sessions.previous.disabled = !canGoPrevious(state.sessionPaging);
    sessions.next.disabled = !canGoNext(state.sessionPaging);
    paintSessionRail(sessions.list, state.sessions, state.selectedSessionKey, (sessionKey) => {
      void selectSession(sessionKey);
    });
  };

  const paintMiddle = (): void => {
    const searching = isSearchActive(state.search.query);
    setText(timeline.title, searching ? "Search results" : "Conversation");
    const paging = searching
      ? searchCanPage(state.search)
      : {
          previous: canGoPrevious(state.exchangePaging),
          next: canGoNext(state.exchangePaging),
        };
    timeline.previous.disabled = !paging.previous || state.search.loading;
    timeline.next.disabled = !paging.next || state.search.loading;
    setText(
      timeline.meta,
      searching ? searchSummary(state.search) : pageRangeLabel(state.exchangePaging),
    );
    if (searching) {
      const showEmpty = state.searchHits.length === 0;
      timeline.empty.hidden = !showEmpty;
      setText(timeline.empty, showEmpty ? searchSummary(state.search) : "");
      paintSearchTimeline(timeline.list, state.searchHits, state.selectedEventKey, (hit) => {
        void selectSearchHit(hit);
      });
      return;
    }
    if (!state.selectedSessionKey) {
      timeline.empty.hidden = false;
      setText(timeline.empty, "Select a session to load exchanges");
      paintExchangeTimeline(timeline.list, [], state.selectedEventKey, () => undefined);
      return;
    }
    if (state.exchanges.length === 0) {
      timeline.empty.hidden = false;
      setText(timeline.empty, "No exchanges in this session");
      paintExchangeTimeline(timeline.list, [], state.selectedEventKey, () => undefined);
      return;
    }
    timeline.empty.hidden = true;
    setText(timeline.empty, "");
    paintExchangeTimeline(
      timeline.list,
      state.exchanges,
      state.selectedEventKey,
      (exchange, eventKey) => {
        void selectExchangeMessage(exchange, eventKey);
      },
    );
  };

  const paintPeerHealth = (): void => {
    paintPeerHealthView(
      peerHealth,
      presentPeerHealth({
        snapshot: state.peerHealth,
        error: state.peerHealthError,
        loading: state.peerHealthLoading,
        busy: state.peerHealthBusy,
        handoffLabel: state.handoffLabel,
        chimeStatus: state.chimeStatus,
      }),
      (incidentId) => {
        void acknowledgeIncident(incidentId);
      },
    );
    paintAttention();
  };

  const paintPeerActivity = (): void => {
    paintPeerActivityView(
      peerActivity,
      presentPeerActivity({
        snapshot: state.peerActivity,
        error: state.peerActivityError,
        loading: state.peerActivityLoading,
      }),
    );
    paintAttention();
  };

  const paintEvent = (): void => {
    if (state.eventLoading) {
      setText(inspector.eventEmpty, "Loading exact event content\u2026");
      inspector.eventEmpty.hidden = false;
      inspector.eventMeta.replaceChildren();
      setText(inspector.eventBody, "");
      return;
    }
    if (state.eventError) {
      setText(inspector.eventEmpty, state.eventError);
      inspector.eventEmpty.hidden = false;
      inspector.eventMeta.replaceChildren();
      setText(inspector.eventBody, "");
      return;
    }
    if (!state.event) {
      setText(inspector.eventEmpty, "Select a request, completion, or search hit");
      inspector.eventEmpty.hidden = false;
      inspector.eventMeta.replaceChildren();
      setText(inspector.eventBody, "");
      return;
    }
    inspector.eventEmpty.hidden = true;
    setText(inspector.eventEmpty, "");
    renderEventMeta(inspector.eventMeta, state.event);
    setText(inspector.eventBody, exactEventBody(state.event));
  };

  const paint = (): void => {
    paintChrome();
    paintPeerActivity();
    paintPeerHealth();
    paintSessions();
    paintMiddle();
    paintEvent();
  };

  const syncSettingsForm = (): void => {
    fillMonitorOptions(inspector.monitor, state.monitors, state.settings.monitorId);
    inspector.corner.value = state.settings.corner;
    desktopMode.value = state.settings.desktopMode;
    if (document.activeElement !== inspector.offsetX) {
      inspector.offsetX.value = String(state.settings.offsetX);
    }
    if (document.activeElement !== inspector.offsetY) {
      inspector.offsetY.value = String(state.settings.offsetY);
    }
    if (document.activeElement !== inspector.width) {
      inspector.width.value = String(state.settings.width);
    }
    if (document.activeElement !== inspector.height) {
      inspector.height.value = String(state.settings.height);
    }
  };

  const resetLists = (): void => {
    sourceEpoch += 1;
    state.sessions = [];
    state.sessionPaging = resetPaging();
    state.selectedSessionKey = null;
    state.exchanges = [];
    state.exchangePaging = resetPaging();
    state.selectedExchangeKey = null;
    state.selectedEventKey = null;
    state.search = createSearchState();
    state.searchHits = [];
    timeline.input.value = "";
    state.event = null;
    state.eventLoading = false;
    state.eventError = null;
  };

  const loadSessions = async (): Promise<void> => {
    const token = ++sessionLoad;
    const epoch = sourceEpoch;
    try {
      const page = await api.listSessions(state.sessionPaging.cursor, state.sessionPaging.limit);
      if (!alive || token !== sessionLoad || epoch !== sourceEpoch) {
        return;
      }
      state.sessions = page.items;
      state.sessionPaging = applyPageResult(state.sessionPaging, {
        nextCursor: page.nextCursor,
        total: page.total,
        itemCount: page.items.length,
      });
    } catch {
      if (!alive || token !== sessionLoad || epoch !== sourceEpoch) {
        return;
      }
      state.loadError = loadErrorLabel("load sessions");
    }
  };

  const loadExchanges = async (): Promise<void> => {
    if (!state.selectedSessionKey) {
      state.exchanges = [];
      state.exchangePaging = resetPaging();
      return;
    }
    const sessionKey = state.selectedSessionKey;
    const token = ++exchangeLoad;
    const epoch = sourceEpoch;
    try {
      const page = await api.listExchanges(
        sessionKey,
        state.exchangePaging.cursor,
        state.exchangePaging.limit,
      );
      if (
        !alive ||
        token !== exchangeLoad ||
        epoch !== sourceEpoch ||
        state.selectedSessionKey !== sessionKey
      ) {
        return;
      }
      state.exchanges = page.items;
      state.exchangePaging = applyPageResult(state.exchangePaging, {
        nextCursor: page.nextCursor,
        total: page.total,
        itemCount: page.items.length,
      });
    } catch {
      if (!alive || token !== exchangeLoad || epoch !== sourceEpoch) {
        return;
      }
      state.loadError = loadErrorLabel("load exchanges");
    }
  };

  const loadSearch = async (): Promise<void> => {
    if (!isSearchActive(state.search.query)) {
      state.searchHits = [];
      state.search.loading = false;
      return;
    }
    const query = state.search.query;
    const token = ++searchLoad;
    const epoch = sourceEpoch;
    state.search.loading = true;
    state.search.error = null;
    paintMiddle();
    try {
      const page = await api.search(query, state.search.paging.cursor, state.search.paging.limit);
      if (!alive || token !== searchLoad || epoch !== sourceEpoch || state.search.query !== query) {
        return;
      }
      state.searchHits = page.items;
      state.search.total = page.total;
      state.search.paging = applyPageResult(state.search.paging, {
        nextCursor: page.nextCursor,
        total: page.total,
        itemCount: page.items.length,
      });
      state.search.loading = false;
      state.search.error = null;
    } catch {
      if (!alive || token !== searchLoad || epoch !== sourceEpoch) {
        return;
      }
      state.search.loading = false;
      state.search.error = loadErrorLabel("search event content");
      state.searchHits = [];
    }
  };

  const loadPeerHealth = async (): Promise<void> => {
    if (state.peerHealthBusy) {
      return;
    }
    const token = ++healthLoad;
    if (!state.peerHealth) {
      state.peerHealthLoading = true;
    }
    try {
      const snapshot = await api.getPeerHealth();
      if (!alive || token !== healthLoad) {
        return;
      }
      state.peerHealth = snapshot;
      state.peerHealthError = null;
      state.peerHealthLoading = false;
    } catch {
      if (!alive || token !== healthLoad) {
        return;
      }
      state.peerHealthLoading = false;
      state.peerHealthError = loadErrorLabel("load peer health");
    }
  };

  const loadPeerActivity = async (): Promise<boolean> => {
    const token = ++activityLoad;
    if (!state.peerActivity) {
      state.peerActivityLoading = true;
    }
    try {
      const snapshot = await api.getPeerActivity();
      if (!alive || token !== activityLoad) {
        return false;
      }
      const revision = peerActivityRevision(snapshot);
      const changed = revision !== state.peerActivityRevision || state.peerActivityError != null;
      state.peerActivity = snapshot;
      state.peerActivityRevision = revision;
      state.peerActivityError = null;
      state.peerActivityLoading = false;
      return changed;
    } catch {
      if (!alive || token !== activityLoad) {
        return false;
      }
      const error = loadErrorLabel("load peer activity");
      const changed = state.peerActivityError !== error || state.peerActivityLoading;
      state.peerActivityLoading = false;
      state.peerActivityError = error;
      return changed;
    }
  };

  const applyPeerHealthSnapshot = (snapshot: PeerHealthSnapshot, token: number): boolean => {
    if (!alive || token !== healthLoad) {
      return false;
    }
    state.peerHealth = snapshot;
    state.peerHealthError = null;
    state.peerHealthLoading = false;
    return true;
  };

  const applyHandoffSelection = (selection: HandoffSelection): void => {
    const applied = applyOpenHandoff(selection, state.peerHealth);
    state.handoffLabel = applied.label.length > 0 ? applied.label : null;
    if (!applied.replaceEvent || !applied.event) {
      return;
    }
    state.event = applied.event;
    state.selectedEventKey = applied.event.eventKey;
    state.selectedSessionKey = applied.event.sessionKey;
    state.selectedExchangeKey = applied.event.exchangeKey;
    state.eventLoading = false;
    state.eventError = null;
  };

  const revealEvent = (): void => {
    const active = document.activeElement;
    const insideActivity = active instanceof Node && inspector.panels.activity.contains(active);
    inspector.activate("event");
    if (!insideActivity || isFocusLocked(active)) {
      return;
    }
    const selected = timeline.list.querySelector<HTMLElement>('[aria-pressed="true"], [aria-selected="true"]');
    if (selected) {
      selected.focus();
      return;
    }
    inspector.eventBody.focus();
  };

  const openLatestHandoff = async (): Promise<void> => {
    if (state.peerHealthBusy) {
      return;
    }
    state.peerHealthBusy = true;
    paintPeerHealth();
    try {
      const selection = await api.openLatestHandoff();
      if (!alive) {
        return;
      }
      applyHandoffSelection(selection);
      state.peerHealthError = null;
      const exchangeVisible = state.exchanges.some(
        (item) => item.exchangeKey === state.selectedExchangeKey,
      );
      if (state.selectedSessionKey && !exchangeVisible && !isSearchActive(state.search.query)) {
        state.exchangePaging = resetPaging();
        await loadExchanges();
      }
    } catch {
      if (!alive) {
        return;
      }
      state.peerHealthError = loadErrorLabel("open the latest handoff");
    } finally {
      if (alive) {
        state.peerHealthBusy = false;
        paintPeerHealth();
        paintEvent();
        paintMiddle();
        if (state.peerHealthError == null) {
          revealEvent();
        }
      }
    }
  };

  const acknowledgeIncident = async (incidentId: string): Promise<void> => {
    if (state.peerHealthBusy) {
      return;
    }
    const token = ++healthLoad;
    state.peerHealthBusy = true;
    paintPeerHealth();
    try {
      const snapshot = await api.acknowledgePeerIncident(incidentId);
      if (!applyPeerHealthSnapshot(snapshot, token)) {
        return;
      }
    } catch {
      if (!alive || token !== healthLoad) {
        return;
      }
      state.peerHealthError = loadErrorLabel("acknowledge a peer-health incident");
    } finally {
      if (alive && token === healthLoad) {
        state.peerHealthBusy = false;
        paintPeerHealth();
      }
    }
  };

  const loadEvent = async (eventKey: string): Promise<EventContent | null> => {
    state.selectedEventKey = eventKey;
    state.eventLoading = true;
    state.eventError = null;
    inspector.activate("event");
    paintEvent();
    paintMiddle();
    try {
      const content = await api.getEventContent(eventKey);
      if (!alive || state.selectedEventKey !== eventKey) {
        return null;
      }
      state.event = content;
      state.eventLoading = false;
      state.eventError = content ? null : "Event content is unavailable";
      paintEvent();
      return content;
    } catch {
      if (!alive || state.selectedEventKey !== eventKey) {
        return null;
      }
      state.event = null;
      state.eventLoading = false;
      state.eventError = loadErrorLabel("load event content");
    }
    paintEvent();
    return null;
  };

  const openWidgetEvent = async (eventKey: string): Promise<void> => {
    const content = await loadEvent(eventKey);
    if (!alive || !content || state.selectedEventKey !== eventKey) {
      return;
    }
    const needsExchangeLoad =
      !isSearchActive(state.search.query) &&
      (state.selectedSessionKey !== content.sessionKey ||
        !state.exchanges.some((item) => item.exchangeKey === content.exchangeKey));
    state.selectedSessionKey = content.sessionKey;
    state.selectedExchangeKey = content.exchangeKey;
    if (needsExchangeLoad) {
      state.exchangePaging = resetPaging();
      await loadExchanges();
    }
    if (alive && state.selectedEventKey === eventKey) {
      paint();
    }
  };

  const selectSession = async (sessionKey: string): Promise<void> => {
    state.selectedSessionKey = sessionKey;
    state.exchangePaging = resetPaging();
    state.selectedExchangeKey = null;
    if (!isSearchActive(state.search.query)) {
      state.selectedEventKey = null;
      state.event = null;
      state.eventError = null;
    }
    await loadExchanges();
    paint();
  };

  const selectExchangeMessage = async (
    exchange: ExchangeSummary,
    eventKey: string,
  ): Promise<void> => {
    state.selectedSessionKey = exchange.sessionKey;
    state.selectedExchangeKey = exchange.exchangeKey;
    await loadEvent(eventKey);
    paint();
  };

  const selectSearchHit = async (hit: SearchHit): Promise<void> => {
    state.selectedSessionKey = hit.sessionKey;
    state.selectedExchangeKey = hit.exchangeKey;
    await loadEvent(hit.eventKey);
    paint();
  };

  const applyStatus = async (
    status: ViewerStatus,
    reload: "reset" | "refresh" | "none",
  ): Promise<"reset" | "refresh" | "none"> => {
    const previous = state.status;
    state.status = status;
    state.loadError = null;
    const plan = reload === "none" ? dataRefreshPlan(previous, status) : reload;
    if (plan === "reset") {
      resetLists();
      await loadSessions();
    } else if (plan === "refresh") {
      await loadSessions();
      await loadExchanges();
      if (isSearchActive(state.search.query)) {
        await loadSearch();
      }
    }
    return plan;
  };

  const removeSource = async (path: string): Promise<void> => {
    if (state.selectingLog) {
      return;
    }
    state.selectingLog = true;
    paintChrome();
    try {
      const status = await api.removeEventLog(path);
      if (!alive) {
        return;
      }
      state.settings = await api.getSettings();
      if (!alive) {
        return;
      }
      state.settingsDirty = false;
      await applyStatus(status, "reset");
      state.loadError = null;
    } catch {
      if (alive) {
        state.loadError = loadErrorLabel("remove an event log");
      }
    } finally {
      state.selectingLog = false;
      if (alive) {
        paint();
      }
    }
  };

  const bootstrap = async (): Promise<void> => {
    try {
      const [status, settings, monitors] = await Promise.all([
        api.getStatus(),
        api.getSettings(),
        api.listMonitors(),
      ]);
      if (!alive) {
        return;
      }
      state.settings = settings;
      state.monitors = monitors;
      await applyStatus(status, "reset");
    } catch {
      if (!alive) {
        return;
      }
      state.loadError = loadErrorLabel("load viewer status");
    }
    paint();
  };

  timeline.form.addEventListener("submit", (event) => {
    event.preventDefault();
    const next = submitSearch(state.search, timeline.input.value);
    state.search = next;
    state.searchHits = [];
    if (!isSearchActive(next.query)) {
      paintMiddle();
      return;
    }
    void loadSearch().then(() => {
      if (alive) {
        paintMiddle();
      }
    });
  });

  sessions.previous.addEventListener("click", () => {
    const next = requestPreviousPage(state.sessionPaging);
    if (!next) {
      return;
    }
    state.sessionPaging = next;
    void loadSessions().then(() => alive && paintSessions());
  });
  sessions.next.addEventListener("click", () => {
    const next = requestNextPage(state.sessionPaging);
    if (!next) {
      return;
    }
    state.sessionPaging = next;
    void loadSessions().then(() => alive && paintSessions());
  });
  timeline.previous.addEventListener("click", () => {
    if (isSearchActive(state.search.query)) {
      const next = requestPreviousPage(state.search.paging);
      if (!next) {
        return;
      }
      state.search.paging = next;
      void loadSearch().then(() => alive && paintMiddle());
      return;
    }
    const next = requestPreviousPage(state.exchangePaging);
    if (!next) {
      return;
    }
    state.exchangePaging = next;
    void loadExchanges().then(() => alive && paintMiddle());
  });
  timeline.next.addEventListener("click", () => {
    if (isSearchActive(state.search.query)) {
      const next = requestNextPage(state.search.paging);
      if (!next) {
        return;
      }
      state.search.paging = next;
      void loadSearch().then(() => alive && paintMiddle());
      return;
    }
    const next = requestNextPage(state.exchangePaging);
    if (!next) {
      return;
    }
    state.exchangePaging = next;
    void loadExchanges().then(() => alive && paintMiddle());
  });

  inspector.selectLog.addEventListener("click", () => {
    if (state.selectingLog) {
      return;
    }
    state.selectingLog = true;
    paintChrome();
    void (async () => {
      try {
        const status = await api.selectEventLog();
        if (!alive) {
          return;
        }
        const settings = await api.getSettings();
        if (!alive) {
          return;
        }
        state.settings = settings;
        state.settingsDirty = false;
        await applyStatus(status, "reset");
      } catch {
        if (alive) {
          state.loadError = loadErrorLabel("add an event log");
        }
      } finally {
        state.selectingLog = false;
        if (alive) {
          paint();
        }
      }
    })();
  });

  inspector.widgetVisible.addEventListener("change", () => {
    const visible = inspector.widgetVisible.checked;
    void (async () => {
      try {
        const status = await api.setWidgetVisible(visible);
        if (!alive) {
          return;
        }
        state.status = status;
      } catch {
        if (alive) {
          state.loadError = loadErrorLabel("update widget visibility");
        }
      }
      if (alive) {
        paintChrome();
      }
    })();
  });

  inspector.launchAtLogin.addEventListener("change", () => {
    const enabled = inspector.launchAtLogin.checked;
    void (async () => {
      try {
        const settings = await api.setLaunchAtLogin(enabled);
        if (!alive) {
          return;
        }
        state.settings = settings;
      } catch {
        if (alive) {
          state.loadError = loadErrorLabel("update launch at login");
        }
      }
      if (alive) {
        paintChrome();
      }
    })();
  });

  inspector.settingsForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const parsed = readSettingsForm(state.settings, inspector);
    if (!parsed) {
      state.loadError = "Placement values must be finite numbers";
      paintChrome();
      return;
    }
    parsed.desktopMode = desktopMode.value === "passive" ? "passive" : "interactive";
    state.savingSettings = true;
    paintChrome();
    void (async () => {
      try {
        const settings = await api.saveSettings(parsed);
        if (!alive) {
          return;
        }
        state.settings = settings;
        state.settingsDirty = false;
        state.loadError = null;
      } catch {
        if (alive) {
          state.loadError = loadErrorLabel("save settings");
        }
      } finally {
        state.savingSettings = false;
        if (alive) {
          paintChrome();
        }
      }
    })();
  });
  inspector.settingsForm.addEventListener("input", () => {
    state.settingsDirty = true;
  });

  retryInteractive.addEventListener("click", () => {
    retryInteractive.disabled = true;
    void api
      .retryInteractiveMode()
      .then((status) => {
        if (!alive) {
          return;
        }
        state.status = status;
        state.loadError = null;
      })
      .catch(() => {
        if (alive) {
          state.loadError = loadErrorLabel("retry interactive desktop mode");
        }
      })
      .finally(() => {
        if (alive) {
          paintChrome();
        }
      });
  });

  exit.addEventListener("click", () => {
    if (state.exiting) {
      return;
    }
    state.exiting = true;
    paintChrome();
    void api.exit().catch(() => {
      if (!alive) {
        return;
      }
      state.exiting = false;
      state.loadError = loadErrorLabel("exit");
      paintChrome();
    });
  });

  peerHealth.muted.addEventListener("change", () => {
    if (state.peerHealthBusy) {
      peerHealth.muted.checked = state.peerHealth?.muted === true;
      return;
    }
    const muted = peerHealth.muted.checked;
    const token = ++healthLoad;
    state.peerHealthBusy = true;
    paintPeerHealth();
    void (async () => {
      try {
        const snapshot = await api.setPeerHealthMuted(muted);
        if (!applyPeerHealthSnapshot(snapshot, token)) {
          return;
        }
      } catch {
        if (!alive || token !== healthLoad) {
          return;
        }
        state.peerHealthError = loadErrorLabel("update peer-health mute");
      } finally {
        if (alive && token === healthLoad) {
          state.peerHealthBusy = false;
          paintPeerHealth();
        }
      }
    })();
  });

  peerHealth.testChime.addEventListener("click", () => {
    if (state.peerHealthBusy) {
      return;
    }
    state.peerHealthBusy = true;
    paintPeerHealth();
    void (async () => {
      try {
        await api.testPeerHealthChime();
        if (!alive) {
          return;
        }
        state.chimeStatus = TEST_CHIME_REQUESTED_LABEL;
        state.peerHealthError = null;
      } catch {
        if (!alive) {
          return;
        }
        state.peerHealthError = loadErrorLabel("request a peer-health test chime");
      } finally {
        if (alive) {
          state.peerHealthBusy = false;
          paintPeerHealth();
        }
      }
    })();
  });

  peerHealth.openHandoff.addEventListener("click", () => {
    void openLatestHandoff();
  });

  let unlistenHandoff: UnlistenFn | null = null;
  let unlistenWidgetOpen: UnlistenFn | null = null;
  if ("__TAURI_INTERNALS__" in window) {
    void listen("peer-health-open-handoff", () => {
      void openLatestHandoff();
    }).then((unlisten) => {
      if (alive) {
        unlistenHandoff = unlisten;
      } else {
        unlisten();
      }
    });
    void listen<{ eventKey: string }>("widget-open-exchange", (event) => {
      if (typeof event.payload.eventKey === "string" && event.payload.eventKey.length > 0) {
        void openWidgetEvent(event.payload.eventKey);
      }
    }).then((unlisten) => {
      if (alive) {
        unlistenWidgetOpen = unlisten;
      } else {
        unlisten();
      }
    });
  }

  const poller = createSingleFlightPoller(async () => {
    try {
      const status = await api.getStatus();
      if (!alive) {
        return;
      }
      const plan = await applyStatus(status, "none");
      if (!alive) {
        return;
      }
      if (plan === "none") {
        paintChrome();
      } else {
        paint();
      }
      return;
    } catch {
      if (!alive) {
        return;
      }
      state.loadError = loadErrorLabel("refresh viewer status");
    }
    paintChrome();
  }, DETAIL_POLL_MS);

  const healthPoller = createSingleFlightPoller(async () => {
    await loadPeerHealth();
    if (alive) {
      paintPeerHealth();
    }
  }, DETAIL_POLL_MS);

  const activityPoller = createSingleFlightPoller(async () => {
    const changed = await loadPeerActivity();
    if (alive && changed) {
      paintPeerActivity();
    }
  }, DETAIL_POLL_MS);

  paint();
  healthPoller.start();
  activityPoller.start();
  void bootstrap().then(() => {
    if (alive) {
      poller.start();
    }
  });

  const stop = (): void => {
    alive = false;
    unlistenHandoff?.();
    unlistenHandoff = null;
    unlistenWidgetOpen?.();
    unlistenWidgetOpen = null;
    poller.stop();
    healthPoller.stop();
    activityPoller.stop();
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function fillMonitorOptions(
  select: HTMLSelectElement,
  monitors: MonitorInfo[],
  selectedId: string | null,
): void {
  const current = document.activeElement === select ? select.value : selectedId ?? "";
  select.replaceChildren(el("option", { text: "Default monitor", attrs: { value: "" } }));
  for (const monitor of monitors) {
    const label = monitor.primary ? `${monitor.name} (primary)` : monitor.name;
    select.append(el("option", { text: label, attrs: { value: monitor.id } }));
  }
  select.value = current;
  if (select.value !== current) {
    select.value = "";
  }
}

function readSettingsForm(
  current: ViewerSettings,
  nodes: {
    monitor: HTMLSelectElement;
    corner: HTMLSelectElement;
    offsetX: HTMLInputElement;
    offsetY: HTMLInputElement;
    width: HTMLInputElement;
    height: HTMLInputElement;
  },
): ViewerSettings | null {
  const offsetX = Number(nodes.offsetX.value);
  const offsetY = Number(nodes.offsetY.value);
  const width = Number(nodes.width.value);
  const height = Number(nodes.height.value);
  if (![offsetX, offsetY, width, height].every(Number.isFinite)) {
    return null;
  }
  const corner = CORNERS.find((item) => item === nodes.corner.value) ?? current.corner;
  return {
    ...current,
    monitorId: nodes.monitor.value === "" ? null : nodes.monitor.value,
    corner,
    offsetX,
    offsetY,
    width,
    height,
  };
}

function desktopRuntimeLabel(status: ViewerStatus | null): string {
  if (!status) {
    return "Desktop mode status is loading";
  }
  const state = status.desktopRuntimeState.replaceAll("-", " ");
  if (!status.desktopFallbackReason) {
    return `Desktop mode: ${state}`;
  }
  return `Desktop mode: ${state}. Reason: ${status.desktopFallbackReason.replaceAll("-", " ")}`;
}

function renderEventMeta(dl: HTMLDListElement, event: EventContent): void {
  dl.replaceChildren();
  const rows: Array<[string, string]> = [
    ["Type", formatEventType(event.eventType)],
    ["Route", formatRoute(event.speaker, event.recipient)],
    ["Timestamp", formatTimestamp(event.timestampMs)],
    ["Status", event.status],
    ["Duration", formatDuration(event.durationMs)],
    ["Event log", event.sourcePath],
    ["Session", event.sessionId],
    ["Exchange", event.exchangeId],
    ["Event", event.eventId],
    ["Length", formatCharacterCount(Array.from(exactEventBody(event)).length)],
  ];
  if (event.error) {
    rows.push(["Parley execution error", event.error]);
  }
  if (event.context) {
    rows.push(
      ["Context source", event.context.source ?? "unknown"],
      ["Context mode", event.context.mode ?? "unknown"],
      [
        "Context offsets",
        `${event.context.fromOffset ?? "unknown"} \u2192 ${event.context.toOffset ?? "unknown"}`,
      ],
      ["Context records", event.context.recordCount?.toString() ?? "unknown"],
      ["Context characters", event.context.characterCount?.toString() ?? "unknown"],
      [
        "Context truncated",
        event.context.truncated == null ? "unknown" : event.context.truncated ? "yes" : "no",
      ],
    );
    if (event.context.recovery) {
      rows.push(["Context recovery", event.context.recovery]);
    }
  }
  for (const [label, value] of rows) {
    dl.append(el("dt", { text: label }), el("dd", { text: value }));
  }
}

function renderSourceList(
  list: HTMLUListElement,
  empty: HTMLParagraphElement,
  sources: SourceStatus[],
  busy: boolean,
  onRemove: (path: string) => void,
): void {
  const active = document.activeElement;
  const activeRecord =
    active instanceof HTMLElement ? active.closest<HTMLElement>("[data-source-path]") : null;
  const restorePath =
    activeRecord && list.contains(activeRecord) && !isFocusLocked(active)
      ? activeRecord.getAttribute("data-source-path")
      : null;
  list.replaceChildren();
  empty.hidden = sources.length > 0;
  for (const source of sources) {
    const remove = el("button", {
      className: "studio-action studio-source-remove",
      text: "Remove",
      attrs: { type: "button" },
    });
    remove.disabled = busy;
    remove.addEventListener("click", () => onRemove(source.path));
    const alias = source.aliasOf ? "; duplicate alias ignored" : "";
    const item = el("li", {
      className: "studio-source-record",
      attrs: { "data-source-path": source.path },
      children: [
        el("p", { className: "studio-source-path", text: source.path }),
        el("p", {
          className: "studio-session-meta",
          text: `${formatSourceState(source.sourceState)}. ${source.sessionCount} sessions; ${source.exchangeCount} exchanges${alias}`,
        }),
        el("p", { className: "studio-diagnostics", text: formatDiagnostics(source.diagnostics) }),
        remove,
      ],
    });
    list.append(item);
  }
  if (restorePath && !isFocusLocked(document.activeElement)) {
    const match = Array.from(list.querySelectorAll<HTMLElement>("[data-source-path]")).find(
      (node) => node.getAttribute("data-source-path") === restorePath,
    );
    match?.querySelector<HTMLButtonElement>("button")?.focus();
  }
}
