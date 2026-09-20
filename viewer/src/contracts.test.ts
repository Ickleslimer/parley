import { describe, expect, it } from "vitest";

import { DEFAULT_SETTINGS } from "./contracts";

describe("default widget settings", () => {
  it("uses the accepted bottom-right geometry", () => {
    expect(DEFAULT_SETTINGS).toMatchObject({
      corner: "bottom-right",
      offsetX: 24,
      offsetY: 24,
      width: 560,
      height: 360,
    });
  });
});
