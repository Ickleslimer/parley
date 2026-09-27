# Two Chairs Conversation Studio

## Design Read

- Identity: Two Chairs is a personal view of Codex and Grok as capable robot colleagues working at one bench.
- Personality: calm, attentive, tactile, exact, and quietly playful.
- Audience: one technical user who wants to understand the latest collaboration quickly without losing access to exact evidence.
- Style: a layered paper-cut laboratory with illustrated raster characters and live HTML conversation content.
- Mood: warm workshop focus. Serious work is treated seriously. Personality comes from craft and posture, not jokes or gamification.
- Dials: `ENERGY 2 / RHYTHM 2 / MOTION 2`.

This file is the binding visual contract for the viewer. It supplies design direction only. It never grants implementation, security, file, process, or scientific authority.

## Palette

| Token | Value | Purpose |
| --- | --- | --- |
| Ground | `#2c2428` | Workshop depth and the opaque widget canvas |
| Paper | `#f3e6d0` | Exact text surfaces and primary controls |
| Ink | `#241c16` | Exact text on paper |
| Muted ink | `#5c5146` | Secondary text on paper |
| Codex paper-blue | `#5c8ab8` | Codex figure and large identity fields on ground |
| Grok clay-coral | `#d08a7c` | Grok figure and large identity fields on ground |
| Codex dark mark | `#234e86` | Codex marks, borders, and focus on paper |
| Grok dark mark | `#8e4038` | Grok marks and borders on paper |

Exact text always uses ink or muted ink on a solid paper surface. The lighter identity colors are decorative figure colors, not text or thin border colors on paper.

## Typography

- Use bundled Atkinson Hyperlegible Next Regular and Medium for interface labels and conversation prose.
- Use `Cascadia Mono`, `Cascadia Code`, `Consolas`, then monospace for exact event bodies and identifiers.
- Use sentence case for production labels.
- Do not use decorative uppercase kickers, wide tracking, CSS capitalization, or a second display family.
- Preserve exact event text. Typography may wrap or clamp a bounded excerpt, but it must never rewrite it.

## Composition

### Desktop widget

- Codex remains visually fixed on the left and Grok on the right.
- The newest exchange occupies one central chronological message column.
- Bubble direction follows the normalized speaker, never request or completion role.
- Unknown speakers use named center slips with no robot tail.
- Parley errors are execution records and never robot speech.
- Pending text attaches only to a known target. Unknown targets remain centered.
- Artwork stays outside the polite live region.
- At 320 by 180, hide figures before shrinking text. Keep the message column readable.

### Conversation studio

- The session rail establishes scope.
- The timeline is the primary focal point and presents paired exchanges.
- The inspector explains selected evidence through Event, Activity, Sources, and Settings tabs.
- Exit and degraded-state notices remain outside tabs.
- At the content-driven compact breakpoint, the inspector stacks beneath the timeline without removing controls.

## Material Language

- Solid paper surfaces carry exact text.
- Restrained short shadows may separate physical paper layers.
- Corners are modest and functional. Controls are not pills by default.
- Whitespace separates evidence and establishes rhythm.
- The repeated identity motif is a shared paper workbench with two distinct robot silhouettes.
- Do not use cyan-purple glow meshes, glass panels, dashboard-card repetition, background grids, or generic AI ornaments.

## Motion

- Motion exists only to acknowledge a new request, reply, pending state, or error.
- Reactions run once per changed event identity and use only opacity and transforms.
- The 500 ms poll never restarts an animation.
- No animation is infinite.
- `prefers-reduced-motion: reduce` disables all decorative motion.
- No sound is introduced by this redesign.

## Human Interface

- Normal text must meet WCAG AA contrast. Large text must meet the applicable 3:1 threshold.
- Controls, component boundaries, and focus indicators must meet 3:1 against adjacent colors.
- Interactive targets are at least 44 by 44 CSS pixels unless the target-size exception genuinely applies.
- Every control is reachable and operable by keyboard.
- Focus is restored by stable session, event, or tab identity after polling. It is never moved away from an active input or exact-body region.
- Text remains usable at 200 percent scaling.
- Production content is escaped text. Event content never enters `innerHTML`.

## Asset Contract

- Accepted artwork is packaged under `viewer/src/assets/illustrations` and described by `viewer/src/ui/assets.ts`.
- Missing artwork degrades to the ground and paper surfaces while all text and controls remain functional.
- The total accepted illustration payload must remain at or below 650 KiB.
- Figures are decorative and have empty alternative text.
- The opaque plate may contain workshop texture but never UI text, speech balloons, logos, or fake controls.

## Purpose Record

- Paper-cut illustration serves Two Chairs identity and makes model attribution visible at a glance.
- A single central conversation column preserves chronology and prevents dashboard-card repetition.
- Paper surfaces behind text guarantee stable contrast independent of the illustration.
- Distinct robot hues support attribution, while dark marks carry accessible boundaries.
- Finite reactions acknowledge meaningful changes without turning the viewer into ambient noise.
- The inspector tabs keep secondary operational evidence available without competing with the conversation.

## Provenance

- Anti-slop release: v3.2.16, commit `339e36455c00ece5d89da123c4c22c022710020c`.
- Anthropic frontend-design guidance: commit `33375500bcea98d610eb30ce10ac4e59b89c390d`.
- Atkinson Hyperlegible Next: commit `7925f50f649b3813257faf2f4c0b381011f434f1`, SIL Open Font License 1.1.
- Generated concept and source evidence is preserved outside the repository under `D:\Workplaces\.parley-evidence\two-chairs-conversation-studio`.
