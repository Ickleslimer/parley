export const LANDMARKS = {
  widgetScene: "widget-scene",
  widgetColumn: "widget-column",
  widgetSurface: "widget-surface",
  widgetSurfaceControls: "widget-surface-controls",
  widgetFeedScroll: "widget-feed-scroll",
  widgetFeedList: "widget-feed-list",
  widgetFeedStatus: "widget-feed-status",
  widgetFeedLiveToggle: "widget-feed-live-toggle",
  widgetFeedJumpLive: "widget-feed-jump-live",
  widgetFeedLoadEarlier: "widget-feed-load-earlier",
  widgetFeedOpenTranscript: "widget-feed-open-transcript",
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
