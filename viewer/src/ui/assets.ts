export type AvatarAgent = "codex" | "grok";
export type AvatarState = "idle" | "working";

export interface AvatarIllustrationAsset {
  agent: AvatarAgent;
  state: AvatarState;
  path: string;
  mediaType: "image/webp";
  byteLength: number;
  sha256: string;
}

export const AVATAR_ILLUSTRATIONS = {
  codexIdle: {
    agent: "codex",
    state: "idle",
    path: new URL("../assets/illustrations/codex-device-idle.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 21_990,
    sha256: "280afc93c849a79fe7c4313cb48c2fc3307a406183c7912f63b9bea8fee19fec",
  },
  codexWorking: {
    agent: "codex",
    state: "working",
    path: new URL("../assets/illustrations/codex-device-working.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 19_628,
    sha256: "3f8eaee9317fd35a93a7238d44458ac4c5186d5680a0507bf5b32a3ee0af1a6b",
  },
  grokIdle: {
    agent: "grok",
    state: "idle",
    path: new URL("../assets/illustrations/grok-device-idle.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 17_726,
    sha256: "9c56430e3fcf250a439b3a17d39339a69ce30e2f6b95b6ac4f523f08956f670f",
  },
  grokWorking: {
    agent: "grok",
    state: "working",
    path: new URL("../assets/illustrations/grok-device-working.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 16_282,
    sha256: "30ce8d0eeac3cdfcfdb21738eba058c9bc5f43e7c29f2c4cea55185ae6c3a30a",
  },
} satisfies Record<string, AvatarIllustrationAsset>;

export const AVATAR_ILLUSTRATION_BYTES = Object.values(AVATAR_ILLUSTRATIONS).reduce(
  (total, asset) => total + asset.byteLength,
  0,
);
