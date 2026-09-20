import type { ViewerStatus, WidgetSnapshot } from "../contracts";

import { presentWidgetSnapshot, type PresentedExchange } from "./excerpt";
import { formatSourceLine, idleWidgetLabel } from "./format";
import { degradedBanner } from "./labels";

export interface WidgetModel extends PresentedExchange {
  banner: string | null;
  sourceLabel: string;
  idleLabel: string | null;
  loadError: string | null;
}

export function widgetModel(args: {
  status: ViewerStatus | null;
  snapshot: WidgetSnapshot | null;
  loadError: string | null;
  utc?: boolean;
}): WidgetModel {
  const presented = args.snapshot
    ? presentWidgetSnapshot(args.snapshot, args.utc === true)
    : { request: null, completion: null, pendingLabel: null, completionHeading: null };
  const hasExchange = Boolean(args.snapshot?.exchangeId || args.snapshot?.request || args.snapshot?.completion);
  const idleLabel = hasExchange || args.loadError
    ? null
    : args.status
      ? idleWidgetLabel(args.status.sourceState)
      : "Connecting\u2026";
  return {
    banner: degradedBanner(args.status),
    sourceLabel: args.status ? formatSourceLine(args.status) : "Connecting\u2026",
    idleLabel,
    loadError: args.loadError,
    ...presented,
  };
}
