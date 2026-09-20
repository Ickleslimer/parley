export interface TimerHandles {
  setInterval: (handler: () => void, ms: number) => number;
  clearInterval: (id: number) => void;
}

const defaultTimers: TimerHandles = {
  setInterval: (handler, ms) => globalThis.setInterval(handler, ms) as unknown as number,
  clearInterval: (id) => globalThis.clearInterval(id),
};

export interface SingleFlightPoller {
  start: () => void;
  stop: () => void;
  inFlight: () => boolean;
  running: () => boolean;
}

export function createSingleFlightPoller(
  tick: () => Promise<void>,
  intervalMs: number,
  timers: TimerHandles = defaultTimers,
): SingleFlightPoller {
  let intervalId: number | null = null;
  let inFlight = false;
  let stopped = true;

  const run = async (): Promise<void> => {
    if (stopped || inFlight) {
      return;
    }
    inFlight = true;
    try {
      await tick();
    } finally {
      inFlight = false;
    }
  };

  return {
    start() {
      if (!stopped && intervalId != null) {
        return;
      }
      stopped = false;
      void run();
      intervalId = timers.setInterval(() => {
        void run();
      }, intervalMs);
    },
    stop() {
      stopped = true;
      if (intervalId != null) {
        timers.clearInterval(intervalId);
        intervalId = null;
      }
    },
    inFlight: () => inFlight,
    running: () => !stopped,
  };
}

export const WIDGET_POLL_MS = 500;
export const DETAIL_POLL_MS = 500;
