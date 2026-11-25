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

Check every screen against these before it ships.

- **One filled primary per view.** The single main action gets
  `variant="primary"`; everything else is default, ghost or link. A form's
  Save sits at the end of its fields and is filled only once there is
  something to save.
- **Accent is not status.** The accent colour marks the current selection and
  the primary action. Success, warning and failure use the `ok`, `warn` and
  `danger` tones only.
- **Mono for code only.** Use the monospace font for paths, hashes, ids,
  commands and tool I/O. Dates, costs, counts, model and profile names are sans;
  numbers that line up in columns get `font-variant-numeric: tabular-nums`.
- **Shade over borders.** Separate regions with the elevation steps
  (`--bg` → `--bg-elevated` → `--bg-elevated-2`), not outlines and rulings.
  Keep 1px borders for inputs, focus rings and dense tables.
- **Hit targets.** Anything clickable is at least 24px on a fine pointer and
  44px on a coarse one, either visibly or through its hit box.
- **Words over glyphs.** Icon-only for universal actions (search, close, more).
  Data gets a word ("25M tokens"), not a symbol.
- **Destructive actions confirm with `ConfirmModal`, `tone="danger"`.** Never
  the native `confirm()`: `scripts/no-native-confirm.sh` and biome's
  `noRestrictedGlobals` reject it. The message says what stops working.
- **Spacing on the scale.** Gaps use `--sp-1`/`2`/`3`/`4`/`6`/`8`. A page
  title sits 16px above its subtitle, and the title group 24px above the
  content.

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
