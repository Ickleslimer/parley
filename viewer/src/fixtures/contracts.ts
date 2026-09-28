export type FixtureSurface = "widget" | "widget-surface" | "detail";

export type FixtureScenario =
  | "short-exchange"
  | "maximum-exchange"
  | "reversed-route"
  | "unknown-agent"
  | "pending"
  | "error"
  | "idle"
  | "missing-image"
  | "search-results"
  | "empty"
  | "source-error"
  | "live"
  | "passive-fallback"
  | "paused-unread"
  | "collapsed"
  | "expanded"
  | "completion";

export type FixtureInspectorTab = "event" | "activity" | "sources" | "settings";

export interface FixtureRequest {
  surface: FixtureSurface;
  scenario: FixtureScenario;
  width: number;
  height: number;
  textScale: 1 | 2;
  reducedMotion: boolean;
  inspectorTab?: FixtureInspectorTab;
}

export const REQUIRED_FIXTURES: readonly FixtureRequest[] = [
  { surface: "widget", scenario: "live", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "live", width: 480, height: 420, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "live", width: 960, height: 720, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "paused-unread", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "collapsed", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "expanded", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "pending", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "completion", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "error", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "unknown-agent", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "empty", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "missing-image", width: 720, height: 560, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "pending", width: 720, height: 560, textScale: 1, reducedMotion: true },
  { surface: "widget-surface", scenario: "live", width: 720, height: 560, textScale: 2, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 480, height: 420, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 960, height: 720, textScale: 1, reducedMotion: false },
  { surface: "detail", scenario: "short-exchange", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "event" },
  { surface: "detail", scenario: "short-exchange", width: 840, height: 560, textScale: 1, reducedMotion: false, inspectorTab: "activity" },
  { surface: "detail", scenario: "search-results", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "sources" },
  { surface: "detail", scenario: "empty", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "settings" },
  { surface: "detail", scenario: "source-error", width: 840, height: 560, textScale: 1, reducedMotion: false, inspectorTab: "event" },
  { surface: "detail", scenario: "maximum-exchange", width: 1120, height: 760, textScale: 1, reducedMotion: true, inspectorTab: "event" },
  { surface: "detail", scenario: "maximum-exchange", width: 1120, height: 760, textScale: 2, reducedMotion: false, inspectorTab: "event" },
] as const;
