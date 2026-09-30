import { el } from "./dom";
import { LANDMARKS } from "./landmarks";
import { type DrawerLayout, type StudioDrawerNodes } from "./studio-seam";

const LAYOUT_CLASS: Record<DrawerLayout, string> = {
  closed: "is-closed",
  wide: "is-wide",
  compact: "is-compact",
};

let drawerSequence = 0;

export function createStudioDrawer(): StudioDrawerNodes {
  const headingId = `studio-drawer-heading-${(drawerSequence += 1)}`;
  const heading = el("h2", {
    className: "studio-focus-drawer-heading",
    text: "Evidence",
    attrs: { id: headingId },
  });
  const close = el("button", {
    className: "studio-focus-close",
    text: "Close",
    attrs: { type: "button" },
  });
  const exactBody = el("pre", { className: "studio-focus-exact" });
  const technicalContent = el("div", { className: "studio-focus-technical" });
  const technicalDetails = el("details", {
    className: "studio-focus-technical-details",
    children: [el("summary", { text: "Technical details" }), technicalContent],
  });
  const utilityHost = el("div", { className: "studio-focus-utility-host" });
  const region = el("section", {
    className: `studio-focus-drawer ${LAYOUT_CLASS.closed}`,
    attrs: {
      "data-region": LANDMARKS.studioDrawer,
      "aria-labelledby": headingId,
    },
    children: [close, heading, exactBody, technicalDetails, utilityHost],
  });

  return {
    region,
    heading,
    close,
    exactBody,
    technicalDetails,
    technicalContent,
    utilityHost,
    setLayoutClass: (layout) => applyLayout(region, layout),
  };
}

function applyLayout(region: HTMLElement, layout: DrawerLayout): void {
  region.classList.remove(LAYOUT_CLASS.closed, LAYOUT_CLASS.wide, LAYOUT_CLASS.compact);
  region.classList.add(LAYOUT_CLASS[layout]);
}
