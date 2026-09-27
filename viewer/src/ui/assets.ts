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
    sha256: "0333bdde09628fa41a57edb2b68ba1dddb74d5f6d18844c4ebbbba3d6e9453095",
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
