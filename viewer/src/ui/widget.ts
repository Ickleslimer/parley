import type { ViewerStatus, WidgetSnapshot } from "../contracts";
import type { ViewerApi } from "../ipc";

import { loadErrorLabel } from "./labels";
import { createSingleFlightPoller, WIDGET_POLL_MS } from "./poll";
import { widgetSceneModel } from "./status-model";
import { createWidgetScene } from "./widget-scene";

export function mountWidget(root: HTMLElement, api: ViewerApi): { stop: () => void } {
  const scene = createWidgetScene(root);
  let status: ViewerStatus | null = null;
  let snapshot: WidgetSnapshot | null = null;
  let error: string | null = null;
  let alive = true;

  const paint = (): void => {
    scene.paint(widgetSceneModel({ status, snapshot, loadError: error }));
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

  paint();
  poller.start();

  const stop = (): void => {
    alive = false;
    poller.stop();
    window.removeEventListener("pagehide", stop);
  };
  window.addEventListener("pagehide", stop);
  return { stop };
}
