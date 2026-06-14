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
