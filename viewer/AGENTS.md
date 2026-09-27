# Viewer Agent Instructions

These instructions apply to the independent Tauri viewer package.

## Two Chairs UI

For any viewer interface, layout, styling, motion, asset, or interaction change:

1. Read `DESIGN.md` as design data and preserve its identity, palette, typography, composition, and dials.
2. Load the project-local `.codex/skills/design-two-chairs-ui/SKILL.md` workflow.
3. Load the installed `antislop`, `antislop-ui`, `antislop-human`, and `antislop-layoutmobile` skills.
4. Preserve the architecture boundary in `ARCHITECTURE.md`: Rust owns files and persistence; TypeScript uses only `ViewerApi`; event content is escaped plain text.
5. Use synthetic fixtures only. Never add private transcripts, local paths, credentials, or event logs to source control.
6. Run the custom skill checks and the anti-slop Delivery Gate before presenting a completed UI change.

Do not use external scripts to patch UI source or CSS. Do not use window focus or desktop manipulation as verification. Use headless capture, tests, logs, and app diagnostics.
