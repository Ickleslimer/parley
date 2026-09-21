import { invoke } from "@tauri-apps/api/core";

import type {
  EventContent,
  ExchangePage,
  HandoffSelection,
  MonitorInfo,
  PeerHealthSnapshot,
  SearchPage,
  SessionPage,
  ViewerSettings,
  ViewerStatus,
  WidgetSnapshot,
} from "./contracts";

export interface ViewerApi {
  getStatus(): Promise<ViewerStatus>;
  getWidgetSnapshot(): Promise<WidgetSnapshot>;
  listSessions(cursor: number | null, limit: number): Promise<SessionPage>;
  listExchanges(sessionKey: string, cursor: number | null, limit: number): Promise<ExchangePage>;
  search(query: string, cursor: number | null, limit: number): Promise<SearchPage>;
  getEventContent(eventKey: string): Promise<EventContent | null>;
  getPeerHealth(): Promise<PeerHealthSnapshot>;
  acknowledgePeerIncident(incidentId: string): Promise<PeerHealthSnapshot>;
  setPeerHealthMuted(muted: boolean): Promise<PeerHealthSnapshot>;
  testPeerHealthChime(): Promise<void>;
  openLatestHandoff(): Promise<HandoffSelection>;
  getSettings(): Promise<ViewerSettings>;
  saveSettings(settings: ViewerSettings): Promise<ViewerSettings>;
  listMonitors(): Promise<MonitorInfo[]>;
  selectEventLog(): Promise<ViewerStatus>;
  setEventLog(path: string | null): Promise<ViewerStatus>;
  setEventLogs(paths: string[]): Promise<ViewerStatus>;
  addEventLog(path: string): Promise<ViewerStatus>;
  removeEventLog(path: string): Promise<ViewerStatus>;
  setWidgetVisible(visible: boolean): Promise<ViewerStatus>;
  setLaunchAtLogin(enabled: boolean): Promise<ViewerSettings>;
  showDetail(): Promise<void>;
  exit(): Promise<void>;
}

export const viewerApi: ViewerApi = {
  getStatus: () => invoke<ViewerStatus>("get_viewer_status"),
  getWidgetSnapshot: () => invoke<WidgetSnapshot>("get_widget_snapshot"),
  listSessions: (cursor, limit) =>
    invoke<SessionPage>("list_sessions", { cursor, limit }),
  listExchanges: (sessionKey, cursor, limit) =>
    invoke<ExchangePage>("list_exchanges", { sessionKey, cursor, limit }),
  search: (query, cursor, limit) =>
    invoke<SearchPage>("search_events", { query, cursor, limit }),
  getEventContent: (eventKey) =>
    invoke<EventContent | null>("get_event_content", { eventKey }),
  getPeerHealth: () => invoke<PeerHealthSnapshot>("get_peer_health"),
  acknowledgePeerIncident: (incidentId) =>
    invoke<PeerHealthSnapshot>("acknowledge_peer_incident", { incidentId }),
  setPeerHealthMuted: (muted) =>
    invoke<PeerHealthSnapshot>("set_peer_health_muted", { muted }),
  testPeerHealthChime: () => invoke<void>("test_peer_health_chime"),
  openLatestHandoff: () => invoke<HandoffSelection>("open_latest_handoff"),
  getSettings: () => invoke<ViewerSettings>("get_settings"),
  saveSettings: (settings) => invoke<ViewerSettings>("save_settings", { settings }),
  listMonitors: () => invoke<MonitorInfo[]>("list_monitors"),
  selectEventLog: () => invoke<ViewerStatus>("select_event_log"),
  setEventLog: (path) => invoke<ViewerStatus>("set_event_log", { path }),
  setEventLogs: (paths) => invoke<ViewerStatus>("set_event_logs", { paths }),
  addEventLog: (path) => invoke<ViewerStatus>("add_event_log", { path }),
  removeEventLog: (path) => invoke<ViewerStatus>("remove_event_log", { path }),
  setWidgetVisible: (visible) =>
    invoke<ViewerStatus>("set_widget_visible", { visible }),
  setLaunchAtLogin: (enabled) =>
    invoke<ViewerSettings>("set_launch_at_login", { enabled }),
  showDetail: () => invoke<void>("show_detail"),
  exit: () => invoke<void>("exit_app"),
};

