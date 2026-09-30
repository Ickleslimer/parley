import type {
  EventContent,
  ExchangeSummary,
  HandoffSelection,
  MonitorInfo,
  PeerActivitySnapshot,
  PeerHealthSnapshot,
  SearchHit,
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
  buildContinuationSection,
  isContinuationReceiptPresented,
  resolveContinuationReceiptEventKey,
} from "./continuation";
import {
  formatCharacterCount,
  formatDiagnostics,
  formatDuration,
  formatEventType,
  formatRoute,
  formatSourceLine,
  formatSourceState,
  formatTimestamp,
} from "./format";
import {
  buildInspector,
  buildStudioDrawerController,
  pendingHandoffCount,
  unreadIncidentCount,
} from "./inspector-tabs";
import { degradedBanner, loadErrorLabel } from "./labels";
import {
  applyPageResult,
  dataRefreshPlan,
  requestNextPage,
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
import { createStudioDrawer } from "./studio-drawer-chrome";
import { createStudioHeader } from "./studio-header";
import type { DrawerLayout, TranscriptMode, UtilityId } from "./studio-seam";
import { createStudioTranscript } from "./studio-transcript-view";
import { exchangeTranscriptRows, isFocusLocked, searchTranscriptRows } from "./timeline";

const CORNERS: Array<ViewerSettings["corner"]> = [
  "top-left",
  "top-right",
  "bottom-left",
  "bottom-right",
];
const CONVERSATION_PAGE_LIMIT = 50;

interface DetailState {
  status: ViewerStatus | null;
  settings: ViewerSettings;
  monitors: MonitorInfo[];
  exchanges: ExchangeSummary[];
  conversationHistoryToken: string | null;
  conversationBeforeExchangeKey: string | null;
  conversationHasEarlier: boolean;
  conversationHasNewer: boolean;
  conversationTotal: number;
  conversationLoading: boolean;
  conversationResetRequired: boolean;
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

  let requestExit = (): void => undefined;
  let openUtility = (_utility: UtilityId): void => undefined;
  let handleSearch = (_query: string): void => undefined;
  let handleLoadEarlier = (): void => undefined;
  let handleJumpLatest = (): void => undefined;
  let handleTranscriptFocus = (_eventKey: string): void => undefined;
  let handleTranscriptActivate = (_eventKey: string, _exchangeKey: string): void => undefined;
  let evaluateContinuationReceipt = (): void => undefined;

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
  const continuation = buildContinuationSection(api, {
    onStatusChange: () => evaluateContinuationReceipt(),
  });
  inspector.activityHost.append(peerActivity.section, peerHealth.section);

  const header = createStudioHeader({
    onExit: () => requestExit(),
    onUtility: (utility) => {
      queueMicrotask(() => openUtility(utility));
    },
  });
  header.status.id = "source-status";
  header.banner.setAttribute("role", "alert");
  const transcript = createStudioTranscript({
    onSearch: (query) => handleSearch(query),
    onLoadEarlier: () => handleLoadEarlier(),
    onJumpLatest: () => handleJumpLatest(),
    onFocus: (eventKey) => handleTranscriptFocus(eventKey),
    onActivate: (eventKey, exchangeKey) => handleTranscriptActivate(eventKey, exchangeKey),
  });
  transcript.scrollport.tabIndex = -1;
  const drawer = createStudioDrawer();
  drawer.exactBody.tabIndex = 0;
  drawer.exactBody.setAttribute("aria-label", "Exact event content");

  for (const utility of ["activity", "sources", "settings"] satisfies UtilityId[]) {
    const panel = inspector.panels[utility];
    panel.removeAttribute("role");
    panel.removeAttribute("id");
    panel.removeAttribute("aria-labelledby");
    panel.classList.add("studio-focus-utility-panel");
    panel.dataset.utility = utility;
    panel.hidden = true;
    drawer.utilityHost.append(panel);
  }

  const loadError = el("p", { className: "studio-load-error", attrs: { role: "alert" } });
  loadError.hidden = true;
  const transcriptColumn = el("main", {
    className: "studio-focus-main",
    children: [transcript.region, continuation.section],
  });
  const workspace = el("div", {
    className: "studio-focus-workspace",
    children: [transcriptColumn, drawer.region],
  });
  const shell = el("div", {
    className: "studio",
    children: [header.region, loadError, workspace],
  });
  root.replaceChildren(shell);

  const state: DetailState = {
    status: null,
    settings: { ...DEFAULT_SETTINGS },
    monitors: [],
    exchanges: [],
    conversationHistoryToken: null,
    conversationBeforeExchangeKey: null,
    conversationHasEarlier: false,
    conversationHasNewer: false,
    conversationTotal: 0,
    conversationLoading: false,
    conversationResetRequired: false,
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
  let conversationLoad = 0;
  let searchLoad = 0;
  let healthLoad = 0;
  let activityLoad = 0;
  let focusedEventKey: string | null = null;
  let transcriptAwayFromLatest = false;
  let renderedEventKey: string | null = null;
  let drawerResizeObserver: ResizeObserver | null = null;

  const compactDrawerQuery =
    typeof window.matchMedia === "function" ? window.matchMedia("(max-width: 53em)") : null;
  const preferredDrawerLayout = (): Exclude<DrawerLayout, "closed"> => {
    const width = root.getBoundingClientRect().width;
    const fontSize = Number.parseFloat(window.getComputedStyle(root).fontSize);
    if (width > 0 && Number.isFinite(fontSize) && fontSize > 0) {
      return width <= 53 * fontSize ? "compact" : "wide";
    }
    return compactDrawerQuery?.matches === true ? "compact" : "wide";
  };
  const resolveTranscriptEvent = (eventKey: string): HTMLElement | null =>
    Array.from(transcript.list.querySelectorAll<HTMLElement>("[data-event-key]")).find(
      (node) => node.dataset.eventKey === eventKey,
    ) ?? null;
  const drawerController = buildStudioDrawerController({
    nodes: drawer,
    background: [header.region, loadError, transcriptColumn],
    resolveEventFocus: resolveTranscriptEvent,
    utilitiesButton: header.utilities.button,
    transcriptFallback: transcript.scrollport,
  });

  const paintChrome = (): void => {
    const bannerText = degradedBanner(state.status);
    const unread = unreadIncidentCount(state.peerHealth);
    const pending = pendingHandoffCount(state.peerActivity);
    header.paint({
      status: state.status ? formatSourceLine(state.status) : "Connecting\u2026",
      banner: bannerText,
      unreadCount: unread,
      pendingCount: pending,
    });
    header.utilities.button.setAttribute(
      "aria-label",
      utilityAttentionLabel(unread, pending),
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
    header.exit.disabled = state.exiting;
    if (!state.settingsDirty) {
      syncSettingsForm();
    }
  };

  const paintMiddle = (scroll: "preserve" | "top" = "preserve"): void => {
    const previousScrollTop = transcript.scrollport.scrollTop;
    const active = document.activeElement;
    const activeRow =
      active instanceof HTMLElement && transcript.list.contains(active)
        ? active.closest<HTMLElement>("[data-event-key]")
        : null;
    const restoreFocusKey = activeRow?.dataset.eventKey ?? null;
    const searching = isSearchActive(state.search.query);
    const paging = searchCanPage(state.search);
    const mode: TranscriptMode = searching
      ? "search"
      : state.conversationHasNewer || state.conversationResetRequired
        ? "historical"
        : "latest";
    let emptyMessage: string | null = null;
    if (searching && state.searchHits.length === 0) {
      emptyMessage = searchSummary(state.search);
    } else if (state.conversationResetRequired) {
      emptyMessage = "Conversation history changed. Reload latest to continue safely";
    } else if (!searching && state.exchanges.length === 0) {
      emptyMessage = state.conversationLoading ? "Loading conversation\u2026" : "No exchanges yet";
    }
    transcript.loadEarlier.textContent = searching
      ? "More search results"
      : "Load earlier messages";
    transcript.loadEarlier.disabled = searching
      ? !paging.next || state.search.loading
      : !state.conversationHasEarlier || state.conversationLoading;
    const showJumpLatest = searching
      ? true
      : state.conversationResetRequired ||
        state.conversationHasNewer ||
        transcriptAwayFromLatest;
    transcript.paint({
      mode,
      rows: searching ? searchTranscriptRows(state.searchHits) : exchangeTranscriptRows(state.exchanges),
      selectedEventKey: state.selectedEventKey,
      focusedEventKey,
      meta: searching ? searchSummary(state.search) : conversationSummary(state),
      emptyMessage,
      hasEarlier: searching ? paging.next : state.conversationHasEarlier,
      showJumpLatest,
      jumpLatestLabel: searching
        ? "Back to conversation"
        : state.conversationResetRequired
          ? "Reload latest"
          : "Jump to latest",
    });
    if (scroll === "top") {
      transcript.scrollport.scrollTop = 0;
    } else if (transcript.scrollport.scrollTop !== previousScrollTop) {
      transcript.scrollport.scrollTop = previousScrollTop;
    }
    if (restoreFocusKey) {
      resolveTranscriptEvent(restoreFocusKey)?.focus();
    }
    evaluateContinuationReceipt();
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
    paintChrome();
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
    paintChrome();
  };

  const paintEvent = (): void => {
    if (drawerController.selected() != null && drawerController.selected() !== "event") {
      return;
    }
    drawer.utilityHost.hidden = true;
    drawer.exactBody.hidden = false;
    if (state.eventLoading) {
      setText(drawer.exactBody, "Loading exact event content\u2026");
      drawer.technicalDetails.hidden = true;
      drawer.technicalContent.replaceChildren();
      return;
    }
    if (state.eventError) {
      setText(drawer.exactBody, state.eventError);
      drawer.technicalDetails.hidden = true;
      drawer.technicalContent.replaceChildren();
      return;
    }
    if (!state.event) {
      setText(drawer.exactBody, "Select a request, completion, or search hit");
      drawer.technicalDetails.hidden = true;
      drawer.technicalContent.replaceChildren();
      return;
    }
    if (renderedEventKey !== state.event.eventKey) {
      renderedEventKey = state.event.eventKey;
      drawer.technicalDetails.open = false;
    }
    setText(drawer.exactBody, exactEventBody(state.event));
    drawer.technicalDetails.hidden = false;
    const metadata = el("dl", { className: "studio-event-meta" });
    renderEventMeta(metadata, state.event);
    drawer.technicalContent.replaceChildren(metadata);
  };

  const paint = (): void => {
    paintChrome();
    paintPeerActivity();
    paintPeerHealth();
    paintMiddle();
    paintEvent();
  };

  const currentTranscriptMode = (): TranscriptMode =>
    isSearchActive(state.search.query)
      ? "search"
      : state.conversationHasNewer || state.conversationResetRequired
        ? "historical"
        : "latest";

  const showEventDrawer = (eventKey: string): void => {
    for (const utility of ["activity", "sources", "settings"] satisfies UtilityId[]) {
      inspector.panels[utility].hidden = true;
    }
    drawer.utilityHost.hidden = true;
    drawer.exactBody.hidden = false;
    drawer.technicalDetails.hidden = false;
    drawer.region.dataset.eventKey = eventKey;
    setText(drawer.heading, "Exact event evidence");
    drawerController.open("event", preferredDrawerLayout(), { kind: "event", eventKey });
    paintEvent();
    evaluateContinuationReceipt();
  };

  const showUtilityDrawer = (utility: UtilityId): void => {
    drawer.region.removeAttribute("data-event-key");
    drawer.exactBody.hidden = true;
    drawer.technicalDetails.hidden = true;
    drawer.utilityHost.hidden = false;
    for (const candidate of ["activity", "sources", "settings"] satisfies UtilityId[]) {
      inspector.panels[candidate].hidden = candidate !== utility;
    }
    setText(drawer.heading, utilityHeading(utility));
    drawerController.open(utility, preferredDrawerLayout(), { kind: "utilities" });
  };
  openUtility = showUtilityDrawer;

  evaluateContinuationReceipt = (): void => {
    const candidate = continuation.receiptCandidate();
    if (!candidate) {
      return;
    }
    const matchingEventKey = resolveContinuationReceiptEventKey(state.exchanges, candidate);
    const exactModalReady =
      matchingEventKey != null &&
      !state.eventLoading &&
      state.eventError == null &&
      state.event?.eventKey === matchingEventKey;
    const modalEventKey = exactModalReady ? drawerController.modalEventKey() : null;
    const row = matchingEventKey ? resolveTranscriptEvent(matchingEventKey) : null;
    if (
      isContinuationReceiptPresented({
        mode: currentTranscriptMode(),
        documentVisible: document.visibilityState === "visible",
        matchingEventKey,
        modalEventKey,
        row,
        scrollport: transcript.scrollport,
      })
    ) {
      continuation.recordVisibleReceipt(candidate);
    }
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
    state.exchanges = [];
    state.conversationHistoryToken = null;
    state.conversationBeforeExchangeKey = null;
    state.conversationHasEarlier = false;
    state.conversationHasNewer = false;
    state.conversationTotal = 0;
    state.conversationLoading = false;
    state.conversationResetRequired = false;
    state.selectedExchangeKey = null;
    state.selectedEventKey = null;
    state.search = createSearchState();
    state.searchHits = [];
    transcript.searchInput.value = "";
    focusedEventKey = null;
    transcriptAwayFromLatest = false;
    state.event = null;
    state.eventLoading = false;
    state.eventError = null;
  };

  const loadConversation = async (
    mode: "latest" | "earlier" | "around" | "refresh",
    anchorExchangeKey: string | null = null,
  ): Promise<boolean> => {
    const token = ++conversationLoad;
    const epoch = sourceEpoch;
    const beforeExchangeKey = mode === "earlier" ? state.conversationBeforeExchangeKey : null;
    const aroundExchangeKey = mode === "around" ? anchorExchangeKey : null;
    if (mode === "earlier" && !beforeExchangeKey) {
      return false;
    }
    state.conversationLoading = true;
    paintMiddle();
    try {
      const page = await api.getConversationPage(
        beforeExchangeKey,
        aroundExchangeKey,
        CONVERSATION_PAGE_LIMIT,
      );
      if (!alive || token !== conversationLoad || epoch !== sourceEpoch) {
        return false;
      }
      const historyChanged =
        state.conversationHistoryToken != null &&
        state.conversationHistoryToken !== page.historyToken;
      if (page.resetRequired || (historyChanged && mode !== "latest")) {
        state.conversationResetRequired = true;
        state.conversationLoading = false;
        return false;
      }
      if (mode === "earlier") {
        state.exchanges = mergeConversationPages(state.exchanges, page.items, "append").items;
        state.conversationBeforeExchangeKey = page.nextBeforeExchangeKey;
        state.conversationHasEarlier = page.hasEarlier;
      } else if (mode === "refresh" && !historyChanged) {
        const refreshed = mergeConversationPages(state.exchanges, page.items, "refresh");
        state.exchanges = refreshed.items;
        state.conversationBeforeExchangeKey = refreshed.keptEarlier
          ? state.conversationBeforeExchangeKey
          : page.nextBeforeExchangeKey;
        state.conversationHasEarlier = refreshed.keptEarlier
          ? state.conversationHasEarlier
          : page.hasEarlier;
        state.conversationHasNewer = false;
      } else {
        state.exchanges = page.items;
        state.conversationBeforeExchangeKey = page.nextBeforeExchangeKey;
        state.conversationHasEarlier = page.hasEarlier;
        state.conversationHasNewer = page.hasNewer;
      }
      state.conversationHistoryToken = page.historyToken;
      state.conversationTotal = page.totalExchanges;
      state.conversationResetRequired = false;
      state.conversationLoading = false;
      state.loadError = null;
      return true;
    } catch {
      if (!alive || token !== conversationLoad || epoch !== sourceEpoch) {
        return false;
      }
      state.conversationLoading = false;
      state.loadError = loadErrorLabel("load the conversation");
      return false;
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
      const exchangeVisible = state.exchanges.some(
        (item) => item.exchangeKey === state.selectedExchangeKey,
      );
      state.search = createSearchState();
      state.searchHits = [];
      transcript.searchInput.value = "";
      if (state.selectedExchangeKey && !exchangeVisible) {
        await loadConversation("around", state.selectedExchangeKey);
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
        if (state.peerHealthError == null && state.selectedEventKey) {
          showEventDrawer(state.selectedEventKey);
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
    showEventDrawer(eventKey);
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
      evaluateContinuationReceipt();
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
    evaluateContinuationReceipt();
    return null;
  };

  const openWidgetEvent = async (eventKey: string): Promise<void> => {
    const content = await loadEvent(eventKey);
    if (!alive || !content || state.selectedEventKey !== eventKey) {
      return;
    }
    const needsConversationLoad = !state.exchanges.some(
      (item) => item.exchangeKey === content.exchangeKey,
    );
    state.selectedExchangeKey = content.exchangeKey;
    state.search = createSearchState();
    state.searchHits = [];
    transcript.searchInput.value = "";
    if (needsConversationLoad) {
      await loadConversation("around", content.exchangeKey);
    }
    if (alive && state.selectedEventKey === eventKey) {
      paint();
    }
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
      await loadConversation("latest");
    } else if (plan === "refresh") {
      if (!state.conversationHasNewer && !state.conversationResetRequired) {
        await loadConversation("refresh");
      }
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

  handleSearch = (query): void => {
    const next = submitSearch(state.search, query);
    state.search = next;
    state.searchHits = [];
    if (!isSearchActive(next.query)) {
      const selectedExchangeKey = state.selectedExchangeKey;
      const selectedVisible = state.exchanges.some(
        (exchange) => exchange.exchangeKey === selectedExchangeKey,
      );
      if (selectedExchangeKey && !selectedVisible) {
        void loadConversation("around", selectedExchangeKey).then(() => {
          if (alive) {
            paintMiddle();
          }
        });
      } else {
        paintMiddle();
      }
      return;
    }
    void loadSearch().then(() => {
      if (alive) {
        paintMiddle();
      }
    });
  };

  handleLoadEarlier = (): void => {
    if (isSearchActive(state.search.query)) {
      const next = requestNextPage(state.search.paging);
      if (!next) {
        return;
      }
      state.search.paging = next;
      void loadSearch().then(() => alive && paintMiddle());
      return;
    }
    void loadConversation("earlier").then(() => alive && paintMiddle());
  };

  handleJumpLatest = (): void => {
    state.search = createSearchState();
    state.searchHits = [];
    transcript.searchInput.value = "";
    transcriptAwayFromLatest = false;
    if (
      state.conversationHasNewer ||
      state.conversationResetRequired ||
      currentTranscriptMode() !== "latest"
    ) {
      void loadConversation("latest").then(() => alive && paintMiddle("top"));
      return;
    }
    paintMiddle("top");
  };

  handleTranscriptFocus = (eventKey): void => {
    focusedEventKey = eventKey;
  };

  handleTranscriptActivate = (eventKey, exchangeKey): void => {
    focusedEventKey = eventKey;
    state.selectedEventKey = eventKey;
    state.selectedExchangeKey = exchangeKey;
    void loadEvent(eventKey).then(() => {
      if (alive) {
        paintMiddle();
      }
    });
  };

  const onTranscriptScroll = (): void => {
    if (currentTranscriptMode() === "latest") {
      const away = transcript.scrollport.scrollTop > 24;
      if (away !== transcriptAwayFromLatest) {
        transcriptAwayFromLatest = away;
        transcript.jumpLatest.hidden = !away;
        setText(transcript.jumpLatest, "Jump to latest");
      }
    }
    evaluateContinuationReceipt();
  };
  const onVisibilityChange = (): void => evaluateContinuationReceipt();
  const onDrawerBreakpointChange = (): void => {
    drawerController.syncLayout(preferredDrawerLayout());
    evaluateContinuationReceipt();
  };
  transcript.scrollport.addEventListener("scroll", onTranscriptScroll);
  document.addEventListener("visibilitychange", onVisibilityChange);
  compactDrawerQuery?.addEventListener("change", onDrawerBreakpointChange);
  if (typeof ResizeObserver === "function") {
    drawerResizeObserver = new ResizeObserver(onDrawerBreakpointChange);
    drawerResizeObserver.observe(root);
  }

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

  requestExit = (): void => {
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
  };

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
  continuation.start();
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
    continuation.stop();
    drawerController.stop();
    transcript.scrollport.removeEventListener("scroll", onTranscriptScroll);
    document.removeEventListener("visibilitychange", onVisibilityChange);
    compactDrawerQuery?.removeEventListener("change", onDrawerBreakpointChange);
    drawerResizeObserver?.disconnect();
    drawerResizeObserver = null;
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}

function utilityAttentionLabel(unread: number, pending: number): string {
  const parts: string[] = [];
  if (unread > 0) {
    parts.push(`${Math.trunc(unread)} unread`);
  }
  if (pending > 0) {
    parts.push(`${Math.trunc(pending)} pending`);
  }
  return parts.length > 0 ? `Utilities, ${parts.join(", ")}` : "Utilities";
}

function utilityHeading(utility: UtilityId): string {
  if (utility === "activity") {
    return "Activity";
  }
  if (utility === "sources") {
    return "Sources";
  }
  return "Settings";
}

function conversationSummary(state: DetailState): string {
  if (state.conversationLoading && state.exchanges.length === 0) {
    return "Loading conversation\u2026";
  }
  if (state.conversationTotal === 0) {
    return "No exchanges";
  }
  const view = state.conversationHasNewer ? "Historical view" : "Latest messages";
  return `${view} \u00b7 ${state.exchanges.length} of ${state.conversationTotal} exchanges`;
}

function mergeConversationPages(
  current: readonly ExchangeSummary[],
  incoming: readonly ExchangeSummary[],
  mode: "append" | "refresh",
): { items: ExchangeSummary[]; keptEarlier: boolean } {
  if (mode === "append") {
    return { items: uniqueExchanges([...current, ...incoming]), keptEarlier: true };
  }
  if (incoming.length === 0) {
    return { items: [], keptEarlier: false };
  }
  const incomingKeys = new Set(incoming.map((exchange) => exchange.exchangeKey));
  const hasOverlap = current.some((exchange) => incomingKeys.has(exchange.exchangeKey));
  if (!hasOverlap) {
    return { items: [...incoming], keptEarlier: false };
  }
  const keptEarlier = current.some((exchange) => !incomingKeys.has(exchange.exchangeKey));
  return {
    items: uniqueExchanges([...incoming, ...current]),
    keptEarlier,
  };
}

function uniqueExchanges(exchanges: readonly ExchangeSummary[]): ExchangeSummary[] {
  const seen = new Set<string>();
  const unique: ExchangeSummary[] = [];
  for (const exchange of exchanges) {
    if (!seen.has(exchange.exchangeKey)) {
      seen.add(exchange.exchangeKey);
      unique.push(exchange);
    }
  }
  return unique;
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
