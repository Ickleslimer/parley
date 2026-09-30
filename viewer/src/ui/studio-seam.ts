export const TRANSCRIPT_MODES = ["latest", "historical", "search"] as const;
export type TranscriptMode = (typeof TRANSCRIPT_MODES)[number];

export const DRAWER_LAYOUTS = ["closed", "wide", "compact"] as const;
export type DrawerLayout = (typeof DRAWER_LAYOUTS)[number];

export const UTILITY_IDS = ["activity", "sources", "settings"] as const;
export type UtilityId = (typeof UTILITY_IDS)[number];

export type TranscriptSide = "request" | "completion";

interface TranscriptRowBase {
  exchangeKey: string;
  speaker: string | null;
  recipient: string | null;
  roleLabel: string;
  timestampMs: number;
  timeLabel: string;
  excerpt: string;
}

export interface TranscriptMessageRow extends TranscriptRowBase {
  kind: "message" | "search";
  eventKey: string;
  side: TranscriptSide;
  pending: false;
}

export interface TranscriptPendingRow extends TranscriptRowBase {
  kind: "pending";
  eventKey: null;
  side: null;
  pending: true;
}

export type TranscriptRowModel = TranscriptMessageRow | TranscriptPendingRow;

export interface TranscriptPaint {
  mode: TranscriptMode;
  rows: readonly TranscriptRowModel[];
  selectedEventKey: string | null;
  focusedEventKey: string | null;
  meta: string;
  emptyMessage: string | null;
  hasEarlier: boolean;
  showJumpLatest: boolean;
  jumpLatestLabel: string;
}

export interface StudioTranscriptCallbacks {
  onSearch: (query: string) => void;
  onLoadEarlier: () => void;
  onJumpLatest: () => void;
  onFocus: (eventKey: string) => void;
  onActivate: (eventKey: string, exchangeKey: string) => void;
}

export interface StudioTranscriptNodes {
  region: HTMLElement;
  scrollport: HTMLDivElement;
  list: HTMLDivElement;
  searchForm: HTMLFormElement;
  searchInput: HTMLInputElement;
  loadEarlier: HTMLButtonElement;
  jumpLatest: HTMLButtonElement;
  meta: HTMLParagraphElement;
  empty: HTMLParagraphElement;
  paint: (input: TranscriptPaint) => void;
}

export interface StudioHeaderPaint {
  status: string;
  banner: string | null;
  unreadCount: number;
  pendingCount: number;
}

export interface StudioMenuNodes {
  button: HTMLButtonElement;
  menu: HTMLElement;
  items: Record<UtilityId, HTMLButtonElement>;
  close: (restoreFocus?: boolean) => void;
}

export interface StudioHeaderNodes {
  region: HTMLElement;
  status: HTMLParagraphElement;
  banner: HTMLParagraphElement;
  utilities: StudioMenuNodes;
  exit: HTMLButtonElement;
  paint: (input: StudioHeaderPaint) => void;
}

export type DrawerContent = "event" | UtilityId;

export interface StudioDrawerNodes {
  region: HTMLElement;
  heading: HTMLHeadingElement;
  close: HTMLButtonElement;
  exactBody: HTMLPreElement;
  technicalDetails: HTMLDetailsElement;
  technicalContent: HTMLElement;
  utilityHost: HTMLElement;
  setLayoutClass: (layout: DrawerLayout) => void;
}
