export const LANDMARKS = {
  widgetScene: "widget-scene",
  widgetLive: "widget-live",
  widgetCodex: "widget-codex",
  widgetGrok: "widget-grok",
  studioSessions: "studio-sessions",
  studioTimeline: "studio-timeline",
  studioSearch: "studio-search",
  studioInspector: "studio-inspector",
  studioTabEvent: "studio-tab-event",
  studioTabActivity: "studio-tab-activity",
  studioTabSources: "studio-tab-sources",
  studioTabSettings: "studio-tab-settings",
  studioExit: "studio-exit",
  studioBanner: "studio-banner",
} as const;

export type Landmark = (typeof LANDMARKS)[keyof typeof LANDMARKS];
