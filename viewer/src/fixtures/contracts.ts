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
  | "historical"
  | "missing-selection"
  | "ambiguous"
  | "passive-fallback";

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
  { surface: "widget", scenario: "short-exchange", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "maximum-exchange", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "short-exchange", width: 320, height: 180, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "reversed-route", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "unknown-agent", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "pending", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "error", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "idle", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget", scenario: "missing-image", width: 560, height: 360, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "historical", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "pending", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "error", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "empty", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "missing-selection", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 320, height: 180, textScale: 1, reducedMotion: false },
  { surface: "widget-surface", scenario: "live", width: 314, height: 348, textScale: 2, reducedMotion: false },
  { surface: "widget-surface", scenario: "passive-fallback", width: 314, height: 348, textScale: 1, reducedMotion: false },
  { surface: "detail", scenario: "short-exchange", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "event" },
  { surface: "detail", scenario: "short-exchange", width: 840, height: 560, textScale: 1, reducedMotion: false, inspectorTab: "activity" },
  { surface: "detail", scenario: "search-results", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "sources" },
  { surface: "detail", scenario: "empty", width: 1120, height: 760, textScale: 1, reducedMotion: false, inspectorTab: "settings" },
  { surface: "detail", scenario: "source-error", width: 840, height: 560, textScale: 1, reducedMotion: false, inspectorTab: "event" },
  { surface: "detail", scenario: "maximum-exchange", width: 1120, height: 760, textScale: 1, reducedMotion: true, inspectorTab: "event" },
  { surface: "detail", scenario: "maximum-exchange", width: 1120, height: 760, textScale: 2, reducedMotion: false, inspectorTab: "event" },
] as const;
