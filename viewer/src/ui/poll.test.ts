import { describe, expect, it } from "vitest";

import { createSingleFlightPoller } from "./poll";

describe("single-flight poller", () => {
  it("does not overlap ticks and clears the timer on stop", async () => {
    const intervals = new Map<number, () => void>();
    let nextId = 1;
    let started = 0;
    let finished = 0;
    let release: (() => void) | undefined;

    const poller = createSingleFlightPoller(
      () => {
        started += 1;
        return new Promise<void>((resolve) => {
          release = () => {
            finished += 1;
            resolve();
          };
        });
      },
      500,
      {
        setInterval(handler) {
          const id = nextId;
          nextId += 1;
          intervals.set(id, handler);
          return id;
        },
        clearInterval(id) {
          intervals.delete(id);
        },
      },
    );

    poller.start();
    expect(started).toBe(1);
    expect(poller.inFlight()).toBe(true);

    for (const handler of intervals.values()) {
      handler();
    }
    await Promise.resolve();
    expect(started).toBe(1);

    release?.();
    await Promise.resolve();
    expect(finished).toBe(1);
    expect(poller.inFlight()).toBe(false);

    poller.stop();
    expect(intervals.size).toBe(0);
    expect(poller.running()).toBe(false);
  });
});
