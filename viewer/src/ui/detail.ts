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

import { button, el, labelledControl, setText } from "./dom";
import { exactEventBody, presentExchange, searchHitHeading } from "./excerpt";
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
import { degradedBanner, loadErrorLabel } from "./labels";
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

  const nodes = buildDetailShell();
  root.replaceChildren(nodes.shell);

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
    peerHealthBusy: false,
    peerActivity: null,
    peerActivityError: null,
    peerActivityLoading: true,
    peerActivityRevision: null,
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

  const paintChrome = (): void => {
    const banner = degradedBanner(state.status);
    nodes.banner.hidden = banner == null;
    setText(nodes.banner, banner ?? "");
    setText(nodes.sourceLine, state.status ? formatSourceLine(state.status) : "Connecting\u2026");
    setText(
      nodes.healthLine,
      state.status ? formatRuntimeHealth(state.status) : "Tray unknown \u00b7 Underlay unknown",
    );
    setText(
      nodes.diagnostics,
      state.status
        ? `${formatDiagnostics(state.status.diagnostics)} \u00b7 generation ${Math.trunc(state.status.generation)} \u00b7 ${Math.trunc(state.status.bytesRead)} bytes`
        : "No diagnostics yet",
    );
    if (state.loadError) {
      nodes.loadError.hidden = false;
      setText(nodes.loadError, state.loadError);
    } else {
      nodes.loadError.hidden = true;
      setText(nodes.loadError, "");
    }
    nodes.widgetVisible.checked = state.status?.widgetVisible === true;
    nodes.launchAtLogin.checked = state.settings.launchAtLogin;
    nodes.selectLog.disabled = state.selectingLog;
    renderSourceList(
      nodes.sourceList,
      nodes.sourceEmpty,
      state.status?.sources ?? [],
      state.selectingLog,
      (path) => void removeSource(path),
    );
    nodes.saveSettings.disabled = state.savingSettings;
    nodes.exit.disabled = state.exiting;
    if (!state.settingsDirty) {
      syncSettingsForm();
    }
  };

  const paintSessions = (): void => {
    setText(nodes.sessionMeta, pageRangeLabel(state.sessionPaging));
    nodes.sessionPrev.disabled = !canGoPrevious(state.sessionPaging);
    nodes.sessionNext.disabled = !canGoNext(state.sessionPaging);
    renderSessionList(nodes.sessionList, state.sessions, state.selectedSessionKey, (sessionKey) => {
      void selectSession(sessionKey);
    });
  };

  const paintMiddle = (): void => {
    const searching = isSearchActive(state.search.query);
    setText(nodes.middleTitle, searching ? "Search results" : "Exchanges");
    setText(nodes.searchMeta, searchSummary(state.search));
    const paging = searching ? searchCanPage(state.search) : {
      previous: canGoPrevious(state.exchangePaging),
      next: canGoNext(state.exchangePaging),
    };
    nodes.middlePrev.disabled = !paging.previous || state.search.loading;
    nodes.middleNext.disabled = !paging.next || state.search.loading;
    if (searching) {
      nodes.middleList.setAttribute("role", "listbox");
      setText(
        nodes.middleEmpty,
        state.searchHits.length === 0 ? searchSummary(state.search) : "",
      );
      nodes.middleEmpty.hidden = state.searchHits.length > 0;
      renderSearchList(nodes.middleList, state.searchHits, state.selectedEventKey, (hit) => {
        void selectSearchHit(hit);
      });
      return;
    }
    nodes.middleList.setAttribute("role", "list");
    if (!state.selectedSessionKey) {
      setText(nodes.middleEmpty, "Select a session to load exchanges");
      nodes.middleEmpty.hidden = false;
      nodes.middleList.replaceChildren();
      return;
    }
    if (state.exchanges.length === 0) {
      setText(nodes.middleEmpty, "No exchanges in this session");
      nodes.middleEmpty.hidden = false;
      nodes.middleList.replaceChildren();
      return;
    }
    nodes.middleEmpty.hidden = true;
    setText(nodes.middleEmpty, "");
    nodes.middleList.setAttribute("role", "list");
    renderExchangeList(
      nodes.middleList,
      state.exchanges,
      state.selectedExchangeKey,
      state.selectedEventKey,
      (exchange, eventKey) => {
        void selectExchangeMessage(exchange, eventKey);
      },
    );
    if (!searching) {
      setText(nodes.searchMeta, pageRangeLabel(state.exchangePaging));
    }
  };

  const paintPeerHealth = (): void => {
    paintPeerHealthView(
      nodes.peerHealth,
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
  };

  const paintPeerActivity = (): void => {
    paintPeerActivityView(
      nodes.peerActivity,
      presentPeerActivity({
        snapshot: state.peerActivity,
        error: state.peerActivityError,
        loading: state.peerActivityLoading,
      }),
    );
  };

  const paintEvent = (): void => {
    if (state.eventLoading) {
      setText(nodes.eventEmpty, "Loading exact event content\u2026");
      nodes.eventEmpty.hidden = false;
      nodes.eventMeta.replaceChildren();
      setText(nodes.eventBody, "");
      return;
    }
    if (state.eventError) {
      setText(nodes.eventEmpty, state.eventError);
      nodes.eventEmpty.hidden = false;
      nodes.eventMeta.replaceChildren();
      setText(nodes.eventBody, "");
      return;
    }
    if (!state.event) {
      setText(nodes.eventEmpty, "Select a request, completion, or search hit");
      nodes.eventEmpty.hidden = false;
      nodes.eventMeta.replaceChildren();
      setText(nodes.eventBody, "");
      return;
    }
    nodes.eventEmpty.hidden = true;
    setText(nodes.eventEmpty, "");
    renderEventMeta(nodes.eventMeta, state.event);
    setText(nodes.eventBody, exactEventBody(state.event));
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
    fillMonitorOptions(nodes.monitor, state.monitors, state.settings.monitorId);
    nodes.corner.value = state.settings.corner;
    if (document.activeElement !== nodes.offsetX) {
      nodes.offsetX.value = String(state.settings.offsetX);
    }
    if (document.activeElement !== nodes.offsetY) {
      nodes.offsetY.value = String(state.settings.offsetY);
    }
    if (document.activeElement !== nodes.width) {
      nodes.width.value = String(state.settings.width);
    }
    if (document.activeElement !== nodes.height) {
      nodes.height.value = String(state.settings.height);
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
    nodes.searchInput.value = "";
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

  const loadEvent = async (eventKey: string): Promise<void> => {
    state.selectedEventKey = eventKey;
    state.eventLoading = true;
    state.eventError = null;
    paintEvent();
    paintMiddle();
    try {
      const content = await api.getEventContent(eventKey);
      if (!alive || state.selectedEventKey !== eventKey) {
        return;
      }
      state.event = content;
      state.eventLoading = false;
      state.eventError = content ? null : "Event content is unavailable";
    } catch {
      if (!alive || state.selectedEventKey !== eventKey) {
        return;
      }
      state.event = null;
      state.eventLoading = false;
      state.eventError = loadErrorLabel("load event content");
    }
    paintEvent();
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

  nodes.searchForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const next = submitSearch(state.search, nodes.searchInput.value);
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

  nodes.sessionPrev.addEventListener("click", () => {
    const next = requestPreviousPage(state.sessionPaging);
    if (!next) {
      return;
    }
    state.sessionPaging = next;
    void loadSessions().then(() => alive && paintSessions());
  });
  nodes.sessionNext.addEventListener("click", () => {
    const next = requestNextPage(state.sessionPaging);
    if (!next) {
      return;
    }
    state.sessionPaging = next;
    void loadSessions().then(() => alive && paintSessions());
  });
  nodes.middlePrev.addEventListener("click", () => {
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
  nodes.middleNext.addEventListener("click", () => {
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

  nodes.selectLog.addEventListener("click", () => {
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

  nodes.widgetVisible.addEventListener("change", () => {
    const visible = nodes.widgetVisible.checked;
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

  nodes.launchAtLogin.addEventListener("change", () => {
    const enabled = nodes.launchAtLogin.checked;
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

  nodes.settingsForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const parsed = readSettingsForm(state.settings, nodes);
    if (!parsed) {
      state.loadError = "Placement values must be finite numbers";
      paintChrome();
      return;
    }
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
  nodes.settingsForm.addEventListener("input", () => {
    state.settingsDirty = true;
  });

  nodes.exit.addEventListener("click", () => {
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

  nodes.peerHealth.muted.addEventListener("change", () => {
    if (state.peerHealthBusy) {
      nodes.peerHealth.muted.checked = state.peerHealth?.muted === true;
      return;
    }
    const muted = nodes.peerHealth.muted.checked;
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

  nodes.peerHealth.testChime.addEventListener("click", () => {
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

  nodes.peerHealth.openHandoff.addEventListener("click", () => {
    void openLatestHandoff();
  });

  let unlistenHandoff: UnlistenFn | null = null;
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
    poller.stop();
    healthPoller.stop();
    activityPoller.stop();
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function buildDetailShell() {
  const logo = el("img", {
    className: "detail-logo",
    attrs: { src: "/parley-icon.ico", alt: "Parley", width: "28", height: "28" },
  });
  const title = el("h1", { className: "detail-title", text: "Parley Conversation Viewer" });
  const sourceLine = el("p", { className: "detail-source", id: "source-status" });
  const healthLine = el("p", { className: "detail-health" });
  const selectLog = button("Add Log", "action", () => undefined, {
    "aria-describedby": "source-status",
  });
  const widgetVisible = el("input", {
    attrs: { type: "checkbox" },
  });
  widgetVisible.id = "widget-visible";
  const launchAtLogin = el("input", {
    attrs: { type: "checkbox" },
  });
  launchAtLogin.id = "launch-at-login";
  const exit = button("Exit", "action action-exit", () => undefined, {
    "aria-label": "Exit Parley Conversation Viewer",
  });
  const actions = el("div", {
    className: "detail-actions",
    children: [
      selectLog,
      labelledControl("Widget visible", widgetVisible, "field field-check"),
      labelledControl("Launch at login", launchAtLogin, "field field-check"),
      exit,
    ],
  });
  const header = el("header", {
    className: "detail-header",
    children: [
      el("div", { className: "detail-brand", children: [logo, title] }),
      el("div", { className: "detail-status", children: [sourceLine, healthLine] }),
      actions,
    ],
  });
  const banner = el("p", {
    className: "detail-banner",
    attrs: { role: "alert" },
  });
  banner.hidden = true;
  const loadError = el("p", { className: "detail-load-error", attrs: { role: "status" } });
  loadError.hidden = true;

  const sessionList = el("ul", {
    className: "record-list",
    attrs: { role: "listbox", "aria-label": "Sessions" },
  });
  const sessionMeta = el("p", { className: "pager-meta" });
  const sessionPrev = button("Previous", "pager-btn", () => undefined);
  const sessionNext = button("Next", "pager-btn", () => undefined);
  const sessionPanel = el("section", {
    className: "panel",
    children: [
      el("h2", { text: "Sessions" }),
      el("div", { className: "panel-body", children: [sessionList] }),
      el("div", {
        className: "pager",
        children: [sessionPrev, sessionMeta, sessionNext],
      }),
    ],
  });

  const middleTitle = el("h2", { text: "Exchanges" });
  const searchInput = el("input", {
    attrs: {
      type: "search",
      name: "query",
      placeholder: "Search exact content",
      "aria-label": "Search exact event content",
      autocomplete: "off",
      spellcheck: "false",
    },
  });
  const searchSubmit = el("button", {
    className: "action",
    text: "Search",
    attrs: { type: "submit" },
  });
  const searchForm = el("form", {
    className: "search-form",
    children: [searchInput, searchSubmit],
  });
  const searchMeta = el("p", { className: "pager-meta" });
  const middleList = el("ul", {
    className: "record-list",
    attrs: { role: "listbox", "aria-label": "Exchanges and search results" },
  });
  const middleEmpty = el("p", { className: "panel-empty" });
  const middlePrev = button("Previous", "pager-btn", () => undefined);
  const middleNext = button("Next", "pager-btn", () => undefined);
  const middlePanel = el("section", {
    className: "panel",
    children: [
      middleTitle,
      searchForm,
      el("div", { className: "panel-body", children: [middleEmpty, middleList] }),
      el("div", { className: "pager", children: [middlePrev, searchMeta, middleNext] }),
    ],
  });

  const eventMeta = el("dl", { className: "event-meta" });
  const eventEmpty = el("p", { className: "panel-empty" });
  const eventBody = el("pre", {
    className: "event-body",
    attrs: { tabindex: "0", "aria-label": "Exact event content" },
  });
  const eventPanel = el("section", {
    className: "panel panel-event",
    children: [
      el("h2", { text: "Event" }),
      eventEmpty,
      eventMeta,
      eventBody,
    ],
  });

  const peerActivity = buildPeerActivitySection();
  const peerHealth = buildPeerHealthSection();

  const sourceList = el("ul", {
    className: "source-list",
    attrs: { "aria-label": "Configured event logs" },
  });
  const sourceEmpty = el("p", { className: "panel-empty", text: "No event logs configured" });
  const sourcesPanel = el("section", {
    className: "panel panel-sources",
    children: [
      el("h2", { text: "Event log sources" }),
      el("div", { className: "panel-body", children: [sourceEmpty, sourceList] }),
    ],
  });

  const diagnostics = el("p", { className: "diagnostics-text" });
  const diagnosticsPanel = el("section", {
    className: "panel",
    children: [el("h2", { text: "Diagnostics" }), diagnostics],
  });

  const monitor = el("select", { attrs: { id: "monitor", "aria-label": "Monitor" } });
  const corner = el("select", { attrs: { id: "corner", "aria-label": "Corner" } });
  for (const value of CORNERS) {
    corner.append(el("option", { text: value, attrs: { value } }));
  }
  const offsetX = numberInput("offset-x", "Offset X");
  const offsetY = numberInput("offset-y", "Offset Y");
  const width = numberInput("width", "Width");
  const height = numberInput("height", "Height");
  const saveSettings = el("button", {
    className: "action",
    text: "Save placement",
    attrs: { type: "submit" },
  });
  const settingsForm = el("form", {
    className: "settings-form",
    children: [
      labelledControl("Monitor", monitor),
      labelledControl("Corner", corner),
      labelledControl("Offset X", offsetX),
      labelledControl("Offset Y", offsetY),
      labelledControl("Width", width),
      labelledControl("Height", height),
      saveSettings,
    ],
  });
  const settingsPanel = el("section", {
    className: "panel",
    children: [el("h2", { text: "Widget placement" }), settingsForm],
  });

  const main = el("div", {
    className: "detail-main",
    children: [sessionPanel, middlePanel, eventPanel],
  });
  const footer = el("div", {
    className: "detail-footer",
    children: [sourcesPanel, diagnosticsPanel, settingsPanel],
  });
  const shell = el("div", {
    className: "detail-layout",
    children: [header, banner, loadError, peerActivity.section, peerHealth.section, main, footer],
  });

  return {
    shell,
    banner,
    loadError,
    sourceLine,
    healthLine,
    selectLog,
    widgetVisible,
    launchAtLogin,
    exit,
    sessionList,
    sessionMeta,
    sessionPrev,
    sessionNext,
    middleTitle,
    searchForm,
    searchInput,
    searchMeta,
    middleList,
    middleEmpty,
    middlePrev,
    middleNext,
    eventMeta,
    eventEmpty,
    eventBody,
    peerActivity,
    peerHealth,
    sourceList,
    sourceEmpty,
    diagnostics,
    monitor,
    corner,
    offsetX,
    offsetY,
    width,
    height,
    saveSettings,
    settingsForm,
  };
}

function numberInput(id: string, label: string): HTMLInputElement {
  return el("input", {
    attrs: {
      id,
      type: "number",
      "aria-label": label,
    },
  });
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

function renderSessionList(
  list: HTMLUListElement,
  sessions: SessionSummary[],
  selectedKey: string | null,
  onSelect: (sessionKey: string) => void,
): void {
  const restoreFocus = list.contains(document.activeElement);
  list.replaceChildren();
  for (const session of sessions) {
    const selected = session.sessionKey === selectedKey;
    const item = el("li", {
      className: selected ? "record selected" : "record",
      attrs: {
        role: "option",
        tabindex: selected || (selectedKey == null && session === sessions[0]) ? "0" : "-1",
        "aria-selected": selected ? "true" : "false",
      },
    });
    const excerpt = el("p", { className: "record-excerpt", text: session.latestExcerpt });
    item.append(
      el("p", { className: "record-title", text: session.sessionId }),
      el("p", {
        className: "record-meta",
        text: `${formatRoute(session.latestSource, session.latestTarget)} \u00b7 ${session.exchangeCount} \u00b7 ${formatTimestamp(session.latestTimestampMs)}`,
      }),
      el("p", { className: "record-source", text: session.sourcePath }),
      excerpt,
    );
    if (session.excerptExtracted) {
      item.append(el("p", { className: "record-flag", text: "Extracted task" }));
    }
    item.addEventListener("click", () => onSelect(session.sessionKey));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        onSelect(session.sessionKey);
      }
    });
    list.append(item);
  }
  bindListKeys(list);
  if (restoreFocus) {
    focusSelected(list);
  }
}

function renderExchangeList(
  list: HTMLUListElement,
  exchanges: ExchangeSummary[],
  selectedExchangeKey: string | null,
  selectedEventKey: string | null,
  onSelect: (exchange: ExchangeSummary, eventKey: string) => void,
): void {
  const restoreFocus = list.contains(document.activeElement);
  list.replaceChildren();
  for (const exchange of exchanges) {
    const presented = presentExchange(exchange);
    const selected = exchange.exchangeKey === selectedExchangeKey;
    const item = el("li", {
      className: selected ? "record selected" : "record",
    });
    item.append(
      el("p", {
        className: "record-title",
        text: `${formatTimestamp(exchange.timestampMs)} \u00b7 ${exchange.exchangeId}`,
      }),
      el("p", { className: "record-source", text: exchange.sourcePath }),
    );
    const request = presented.request;
    if (request) {
      const requestBtn = button(
        `${request.heading}${request.extractedLabel ? ` \u00b7 ${request.extractedLabel}` : ""} \u00b7 ${request.route}`,
        selectedEventKey === request.eventKey ? "record-link selected" : "record-link",
        () => onSelect(exchange, request.eventKey),
      );
      item.append(requestBtn, el("p", { className: "record-excerpt", text: request.excerpt }));
    }
    const completion = presented.completion;
    if (completion) {
      const completionBtn = button(
        `${completion.heading} \u00b7 ${completion.route}`,
        selectedEventKey === completion.eventKey ? "record-link selected" : "record-link",
        () => onSelect(exchange, completion.eventKey),
      );
      item.append(
        completionBtn,
        el("p", { className: "record-excerpt", text: completion.excerpt }),
      );
    } else if (presented.pendingLabel) {
      item.append(el("p", { className: "record-flag", text: presented.pendingLabel }));
    }
    list.append(item);
  }
  if (restoreFocus) {
    const selected = list.querySelector<HTMLElement>(".record.selected .record-link, .record.selected");
    selected?.focus();
  }
}

function renderSearchList(
  list: HTMLUListElement,
  hits: SearchHit[],
  selectedEventKey: string | null,
  onSelect: (hit: SearchHit) => void,
): void {
  const restoreFocus = list.contains(document.activeElement);
  list.replaceChildren();
  for (const hit of hits) {
    const selected = hit.eventKey === selectedEventKey;
    const item = el("li", {
      className: selected ? "record selected" : "record",
      attrs: {
        role: "option",
        tabindex: selected ? "0" : "-1",
        "aria-selected": selected ? "true" : "false",
      },
    });
    item.append(
      el("p", {
        className: "record-title",
        text: `${searchHitHeading(hit.eventType)} \u00b7 ${formatTimestamp(hit.timestampMs)}`,
      }),
      el("p", {
        className: "record-meta",
        text: `${hit.sessionId} \u00b7 match offset ${Math.trunc(hit.matchOffset)}`,
      }),
      el("p", { className: "record-source", text: hit.sourcePath }),
      el("p", { className: "record-excerpt", text: hit.excerpt }),
    );
    item.addEventListener("click", () => onSelect(hit));
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        onSelect(hit);
      }
    });
    list.append(item);
  }
  bindListKeys(list);
  if (restoreFocus) {
    focusSelected(list);
  }
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
  list.replaceChildren();
  empty.hidden = sources.length > 0;
  for (const source of sources) {
    const remove = button("Remove", "source-remove", () => onRemove(source.path));
    remove.disabled = busy;
    const alias = source.aliasOf ? " \u00b7 duplicate alias ignored" : "";
    const item = el("li", {
      className: "source-record",
      children: [
        el("p", { className: "source-path", text: source.path }),
        el("p", {
          className: "record-meta",
          text: `${formatSourceState(source.sourceState)} \u00b7 ${source.sessionCount} sessions \u00b7 ${source.exchangeCount} exchanges${alias}`,
        }),
        el("p", { className: "source-diagnostics", text: formatDiagnostics(source.diagnostics) }),
        remove,
      ],
    });
    list.append(item);
  }
}

function bindListKeys(list: HTMLUListElement): void {
  const options = () => Array.from(list.querySelectorAll<HTMLElement>('[role="option"]'));
  list.onkeydown = (event: KeyboardEvent) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Home" && event.key !== "End") {
      return;
    }
    const items = options();
    if (items.length === 0) {
      return;
    }
    const currentIndex = items.findIndex((item) => item === document.activeElement || item.contains(document.activeElement));
    let nextIndex = currentIndex < 0 ? 0 : currentIndex;
    if (event.key === "ArrowDown") {
      nextIndex = Math.min(items.length - 1, currentIndex + 1);
    } else if (event.key === "ArrowUp") {
      nextIndex = Math.max(0, currentIndex < 0 ? 0 : currentIndex - 1);
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else {
      nextIndex = items.length - 1;
    }
    const next = items[nextIndex];
    if (!next) {
      return;
    }
    event.preventDefault();
    for (const item of items) {
      item.tabIndex = -1;
    }
    next.tabIndex = 0;
    next.focus();
  };
}

function focusSelected(list: HTMLUListElement): void {
  const selected = list.querySelector<HTMLElement>('[aria-selected="true"]');
  (selected ?? list.querySelector<HTMLElement>('[role="option"]'))?.focus();
}
