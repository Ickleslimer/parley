import type { ViewerStatus, WidgetSnapshot, WidgetSurfaceBoundsReport } from "../contracts";
import type { ViewerApi } from "../ipc";

import { loadErrorLabel } from "./labels";
import { LANDMARKS } from "./landmarks";
import { createSingleFlightPoller, WIDGET_POLL_MS } from "./poll";
import { widgetSceneModel } from "./status-model";
import { createWidgetScene } from "./widget-scene";

const MAX_DEVICE_PIXEL_RATIO = 8;
const MAX_BOUNDS_FAILURES = 3;

export function validatedSurfaceBounds(
  rect: Pick<DOMRectReadOnly, "left" | "top" | "width" | "height">,
  viewportWidth: number,
  viewportHeight: number,
  devicePixelRatio: number,
): WidgetSurfaceBoundsReport | null {
  if (
    !Number.isFinite(devicePixelRatio) ||
    devicePixelRatio <= 0 ||
    devicePixelRatio > MAX_DEVICE_PIXEL_RATIO
  ) {
    return null;
  }
  if (!Number.isFinite(viewportWidth) || !Number.isFinite(viewportHeight)) {
    return null;
  }
  const quantize = (value: number): number => {
    if (!Number.isFinite(value)) {
      return Number.NaN;
    }
    return Math.round(value * devicePixelRatio) / devicePixelRatio;
  };
  const left = quantize(rect.left);
  const top = quantize(rect.top);
  const width = quantize(rect.width);
  const height = quantize(rect.height);
  const nextViewportWidth = quantize(viewportWidth);
  const nextViewportHeight = quantize(viewportHeight);
  const values = [left, top, width, height, nextViewportWidth, nextViewportHeight];
  if (values.some((value) => !Number.isFinite(value))) {
    return null;
  }
  if (width <= 0 || height <= 0 || nextViewportWidth <= 0 || nextViewportHeight <= 0) {
    return null;
  }
  const slop = 1 / devicePixelRatio;
  if (left < -slop || top < -slop) {
    return null;
  }
  if (left + width > nextViewportWidth + slop || top + height > nextViewportHeight + slop) {
    return null;
  }
  return {
    left,
    top,
    width,
    height,
    viewportWidth: nextViewportWidth,
    viewportHeight: nextViewportHeight,
    devicePixelRatio,
  };
}

export function mountWidget(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  const scene = createWidgetScene(root);
  let status: ViewerStatus | null = null;
  let snapshot: WidgetSnapshot | null = null;
  let error: string | null = null;
  let alive = true;
  let acceptedSignature: string | null = null;
  let pendingSignature: string | null = null;
  let failedSignature: string | null = null;
  let failureCount = 0;

  const reportBounds = (): void => {
    if (!alive) {
      return;
    }
    const column = root.querySelector<HTMLElement>(`#${LANDMARKS.widgetColumn}`);
    if (!column) {
      return;
    }
    const report = validatedSurfaceBounds(
      column.getBoundingClientRect(),
      window.innerWidth,
      window.innerHeight,
      window.devicePixelRatio,
    );
    if (!report) {
      return;
    }
    const signature = JSON.stringify(report);
    if (signature === acceptedSignature || signature === pendingSignature) {
      return;
    }
    if (signature === failedSignature && failureCount >= MAX_BOUNDS_FAILURES) {
      return;
    }
    if (signature !== failedSignature) {
      failureCount = 0;
    }
    pendingSignature = signature;
    void api.reportWidgetSurfaceBounds(report).then(
      () => {
        if (!alive || pendingSignature !== signature) {
          return;
        }
        acceptedSignature = signature;
        pendingSignature = null;
        failedSignature = null;
        failureCount = 0;
      },
      () => {
        if (!alive || pendingSignature !== signature) {
          return;
        }
        pendingSignature = null;
        failedSignature = signature;
        failureCount += 1;
      },
    );
  };

  const paint = (): void => {
    scene.paint(widgetSceneModel({ status, snapshot, loadError: error }));
    scene.setCovered(status?.desktopRuntimeState === "interactive");
    reportBounds();
  };

  const poller = createSingleFlightPoller(async () => {
    try {
      const [nextStatus, nextSnapshot] = await Promise.all([
        api.getStatus(),
        api.getWidgetSnapshot(),
      ]);
      if (!alive) {
        return;
      }
      status = nextStatus;
      snapshot = nextSnapshot;
      error = null;
    } catch {
      if (!alive) {
        return;
      }
      error = loadErrorLabel("load widget snapshot");
    }
    paint();
  }, WIDGET_POLL_MS);

  const onResize = (): void => {
    reportBounds();
  };

  paint();
  poller.start();
  window.addEventListener("resize", onResize);

  const stop = (): void => {
    alive = false;
    poller.stop();
    window.removeEventListener("resize", onResize);
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}
