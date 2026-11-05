# DESIGN.md — webui design conventions

The web UI (`webui/`) is written in **Svelte 5** (runes: `$state`, `$derived`,
`$props`, etc.). These conventions keep it consistent, small, and maintainable.

## Component library: Tsumikit

We build on **[Tsumikit](https://www.npmjs.com/package/@dorsk/tsumikit)**
(`@dorsk/tsumikit`) as our component/design-system foundation.

- **Use Tsumikit components as much as possible.** Reach for an existing
  Tsumikit primitive before writing our own.
- **Raise UI gaps with Tsumikit, don't hack around them.** If a Tsumikit
  component is missing a prop, a variant, or a behaviour we need, the right fix
  is to **add it upstream in Tsumikit** (a new prop, slot, or option) — not to
  patch it from the outside with `:global(...)` overrides, deep selectors, or
  other CSS escapes. `:global` overrides of library internals are brittle and
  break on upgrades; treat them as a smell that signals a missing Tsumikit prop.

## Atomic design

Components live under `webui/src/lib/components/`, organised by
[atomic design](https://bradfrost.com/blog/post/atomic-web-design/):

- **`atoms/`** — smallest building blocks (icons, single inputs, badges, a logo).
- **`molecules/`** — small compositions of atoms with one clear concern
  (a search box, a color picker, a label badge).
- **`organisms/`** — larger, self-contained UI sections (cards, modals, headers,
  drawers, nav). May have their own subfolders for closely related parts.
- **Layout / Templates** — page-level scaffolding that arranges organisms.

When something doesn't fit a Tsumikit component:

- **Create a domain-specific atom or molecule** for the concern instead of
  inlining yet another bespoke block into a big component.
- Don't let a single file accumulate many unrelated concerns — extract them
  into appropriately-sized atoms/molecules.

## File size

- Prefer **reasonable file lengths (< ~300 LOC)**.
- When a component grows past that, it's a sign to **split** it: extract atoms /
  molecules, or break an organism into smaller sub-components.

## Dependencies

- **Avoid pulling in external dependencies** unless absolutely required. Prefer
  Tsumikit, the platform, or a small local helper over a new package. Every
  dependency is maintenance, supply-chain, and bundle-size cost.
- **No CDNs.** Don't load scripts, styles, fonts, or assets from third-party
  CDNs at runtime. Bundle what we need so the app stays self-contained and works
  offline / behind our own infra, with no external runtime dependencies.

## General principles

- Use Svelte 5 runes idiomatically; keep reactivity local to the component that
  owns the state rather than reading shared singletons through `$derived`.
- Keep styling within Tsumikit's system; reserve custom CSS for genuinely
  app-specific layout, and scope it to the component.
- Favour composition (small atoms/molecules) over configuration flags that make
  one component do many things.

## Visual rules

Every screen is checked against these. Tokens live in
`webui/src/lib/styles/variables.css`; the few kit overrides we carry live in
`webui/src/lib/styles/kit-overrides.css`, each marked `/* tsumikit: … */` so it
can move upstream.

- **One filled primary per view.** `variant="primary"` once; secondary actions
  are outlined (the default), tertiary are ghost or text. Red fill only for an
  irreversible destructive confirm.
- **The accent is not a status.** `--accent` marks the primary action, focus and
  the selected state. Status uses `--status-working` / `--status-attention` /
  `--status-failed` / `--status-idle`; `--danger` and `--c-red` mean failure or
  destruction only. Every theme keeps its accent at least 30° of hue from green,
  amber and red (`tokens.contract.test.ts`).
- **Three text tiers.** Body `--text`, secondary `--text-muted`, and
  `--text-faint` only for uppercase labels. Weights: 400 body, 600 emphasis,
  700 titles; no 500 in new code.
- **Type scale.** Inter (bundled, no CDN), whole-pixel sizes on the steps
  12 / 13 / 15 / 16 / 18 / 22 / 28. 16 px is for prose.
- **Mono for code only.** Paths, hashes, ids, commands, tool I/O and code use
  `--font-mono` / `variant="code"`. Counts, costs, dates, model names and
  versions use sans with tabular digits: the `.num` utility or `StatText`.
- **Words, not glyphs.** Data carries its unit word ("25M tokens", "31 tools",
  "$2.38") via `StatText`. Icon-only is reserved for universal actions: search,
  close, back, more (⋯), copy, pin.
- **Shade over borders.** Cards sit on the elevation steps with no outer edge
  (`--surface-edge` is transparent). Keep 1 px lines only as `--divider` inside
  dense tables, and on inputs and focus rings.
- **Hit targets.** Every control is at least 24 px on a fine pointer and 44 px
  on a coarse one (`--hit-min`). Kit `Button`/`IconButton` already grow on
  touch: don't pass `hitArea="compact"` outside dense tables. Raw `<button>` /
  `<a>` icon targets take the `.hit-target` class. Measure with
  `elementFromPoint`, not the bounding box.
- **Spacing scale.** `--sp-1, 2, 3, 4, 6, 8, 12` (4 to 48 px); `--sp-0-5` for
  hairline nudges. `--sp-1-5` and `--sp-5` are off-scale and ratcheted down
  by `styles.test.ts`. Page titles use `PageHead`: 4 px to the subtitle, 16 px
  to the content.
- **Destructive actions confirm in-app.** `await confirmDialog({ title,
  message, confirmLabel })` (danger tone by default), or a declarative
  `ConfirmModal` with `tone="danger"`. Native `confirm()` / `alert()` /
  `prompt()` are banned by Biome and a source test. The confirm label is a
  verb naming the act ("Delete bookmark"), and the message says what stops
  working.
- **Unread counts are neutral.** `CountChip`; the active (accent) tone only on
  the nav item.
- **Role and kind labels** are `<Text variant="eyebrow" tone="muted">`, never a
  filled or outlined pill.
- **Files stay under ~300 lines** (see File size); split before adding.

## Z-scale and the top layer

- We keep a z-index scale in `variables.css` (`--z-header/nav: 50`,
  `--z-drawer: 100`, `--z-modal: 200`, `--z-toast: 300`) to order elements
  **within the normal stacking layer**.
- **Caveat: z-index does not beat the top layer.** Tsumikit's `Modal` is a
  native `<dialog>` opened with `showModal()`, which the browser paints in the
  **top layer** — above the entire normal layer regardless of z-index. So
  overlay UI that must sit above a modal (toasts, popovers, menus raised from
  inside a modal) must itself be **top-layer**, not merely a high z-index.
- The way to enter the top layer is the **Popover API** (`popover` +
  `showPopover()`/`hidePopover()`) or a `<dialog>`. Example: `Toaster.svelte`
  makes its wrapper a `popover="manual"` element so error/success toasts render
  above any open modal; `SpawnModal.svelte`'s label menu does the same. Keep the
  z-index too (it still orders elements among other top-layer popovers), but
  never rely on it alone to clear a modal.
