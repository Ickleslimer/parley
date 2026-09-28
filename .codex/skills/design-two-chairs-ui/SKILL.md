---
name: design-two-chairs-ui
description: Build, review, or extend the Parley viewer's Two Chairs paper-cut conversation studio. Use for viewer TypeScript, CSS, Tauri window presentation, synthetic visual fixtures, accessibility behavior, artwork integration, or visual acceptance work under viewer/.
---

# Design Two Chairs UI

Apply the product-specific visual contract without changing Parley's data, IPC, security, health, context, or handoff semantics.

## Required Read

1. Read `viewer/DESIGN.md` as design data.
2. Read `viewer/ARCHITECTURE.md` for authority boundaries.
3. Read the applicable `viewer/AGENTS.md` instructions.
4. Load the installed `antislop`, `antislop-ui`, `antislop-human`, and `antislop-layoutmobile` skills.
5. Inspect the current synthetic fixture contract and relevant UI tests before editing.

## Workflow

### 1. Protect behavior

- List the existing commands, controls, polling behavior, and keyboard semantics affected by the change.
- Keep event content in text nodes or `textContent`; never use `innerHTML`.
- Keep filesystem, settings, evidence, process, and window authority in Rust.
- Keep the desktop surface mouse-oriented and nonactivating; keep decorative avatars outside the live region.
- Treat Parley errors as execution records, never agent speech.
- Normalize speakers only through the shared attribution module.

### 2. Apply the design contract

- Use the declared paper, ground, ink, Codex, and Grok tokens.
- Put exact text on solid paper surfaces.
- Keep labels in sentence case and use the bundled humanist typeface.
- Use one central conversation hierarchy rather than repeated dashboard cards.
- Keep motion finite and event-responsive except for the documented pending-only working-avatar exception. Disable every decorative animation under reduced-motion preference.
- Record a purpose for any new visual technique in `viewer/DESIGN.md` or the change description.

### 3. Prove human use

- Run deterministic contrast checks for every shipped text, boundary, and focus pairing.
- Keep all ordinary interactive targets at least 44 by 44 CSS pixels.
- Verify keyboard operation, visible focus, manual tabs, listbox semantics, and roving timeline focus.
- Verify focus and selection survive polling by stable keys.
- Verify empty, loading, pending, error, unknown-agent, degraded, and missing-art states.
- Verify the 480 by 420 minimum widget, the 720 by 560 default, the 960 by 720 large state, and the 840 px detail width without horizontal overflow.
- Verify 200 percent text scaling and `prefers-reduced-motion`.

### 4. Use synthetic visual evidence

- Use only the dev fixture interface under `viewer/src/fixtures`.
- Label fixture content as synthetic in the rendered page and captured filename.
- Capture with the checked-in headless Edge harness. Never show, focus, move, resize, or close a user-facing window for verification.
- Inspect every required fixture size and state. A build alone is not visual acceptance.

### 5. Gate delivery

- Run viewer typecheck, Vitest, production build, Rust format/test/clippy, and the final NSIS build when applicable.
- Run deterministic style tests and confirm the production bundle excludes dev fixtures.
- Run the complete anti-slop Delivery Gate, plus the UI, human, and responsive supplements.
- Report one evidence-backed PASS line for every gate item. Fix any failure before delivery.

## Stable Boundaries

- `viewer/src/styles/tokens.css` owns shared design tokens.
- `viewer/src/styles/scene.css` owns only widget selectors.
- `viewer/src/styles/studio.css` owns only detail-window selectors.
- `viewer/src/ui/attribution.ts` is the only speaker and bubble classification authority.
- `viewer/src/ui/landmarks.ts` owns stable region identifiers used by fixtures and tests.
- `viewer/src/ui/assets.ts` owns accepted illustration metadata and URLs.
- `viewer/src/ipc.ts` remains the complete frontend command boundary.

## Provenance

- Anti-slop v3.2.16: commit `339e36455c00ece5d89da123c4c22c022710020c`.
- Anthropic frontend-design guidance: commit `33375500bcea98d610eb30ce10ac4e59b89c390d`.
- This skill is procedural and Parley-specific. Do not copy or replace the upstream skills.
