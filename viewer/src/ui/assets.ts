export type IllustrationRole = "plate" | "codex" | "grok";

export interface IllustrationAsset {
  role: IllustrationRole;
  path: string;
  mediaType: "image/webp";
  byteLength: number;
  sha256: string;
}

export const ILLUSTRATIONS = {
  plate: {
    role: "plate",
    path: new URL("../assets/illustrations/workshop-plate.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 19_820,
    sha256: "4c8b40fd49bdfcc6f26f0fa97970c759035f04370d250b8f1f7e3f586e83f731",
  },
  codex: {
    role: "codex",
    path: new URL("../assets/illustrations/codex-robot.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 59_010,
    sha256: "0333bdde09628fa41a57edb2b68ba1dddb74d5f6d18844c4ebbba3d6e9453095",
  },
  grok: {
    role: "grok",
    path: new URL("../assets/illustrations/grok-robot.webp", import.meta.url).href,
    mediaType: "image/webp",
    byteLength: 54_342,
    sha256: "818a96aa7c3517332ba7b0a0c8b6668aef1be34e7d91a766c4e395f33cf23a0d",
  },
} satisfies Record<IllustrationRole, IllustrationAsset>;

export const ILLUSTRATION_BYTES = Object.values(ILLUSTRATIONS).reduce(
  (total, asset) => total + asset.byteLength,
  0,
);

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
