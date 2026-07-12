# Web UI design system

How the front-end is layered, and the rules that keep buttons, selects,
surfaces, cards and icons uniform across the whole app. The payoff: change the
*general* appearance (more padding everywhere, a new radius, an extra `aria-*`,
a tweaked palette) by editing **one** place and letting the whole tree inherit
it — never by hand-editing 50 components.

## The layers

Each layer may only reach **one layer down**. Nothing skips a layer (a page
never hand-rolls a button; an organism never emits a raw `<select>`).

```
CSS variables   → variables.css: every colour, space, radius, font, size.
                  No component hard-codes a hex or a pixel.
        ↓
Atoms           → lib/components/atoms/*: the ONLY place a raw HTML primitive
                  (<button>, <select>, <input>, <textarea>, <a>, <h1>-<h6>, and
                  text-bearing <p>/<span>/<label>) is allowed. Each owns its
                  scoped styling from CSS vars and exposes a base shape +
                  props/variants. e.g. Button, Select, Input, Icon, Card, and
                  Heading / Text for ALL copy (headings and body/label/caption).
        ↓
Molecules       → lib/components/molecules/*: combine atoms, or SPECIALIZE one
                  atom via a variant + overrides. Never reimplement a primitive.
                  e.g. IconButton = Button(ghost) + Icon; Field = label + slot;
                  SelectButton = Button(ghost) + Select(ghost).
        ↓
Organisms       → lib/components/organisms/*: purpose-driven assemblies of
                  molecules + atoms. e.g. ConversationDrawer, SpawnModal,
                  SessionCard, Header.
        ↓
Pages / routes  → routes/*: assemble organisms, .ts logic (queries, format,
                  drafts) and layout. Presentation only — no bespoke controls.
```

## Rules

1. **Primitives live in atoms only.** If you're typing `<button>`, `<select>`,
   `<input>`, `<textarea>`, `<a>`, an `<h1>`–`<h6>`, or a text-bearing
   `<p>`/`<span>`/`<label>` anywhere outside `atoms/`, stop — use the atom (or
   add a prop/variant to it). This is the rule that makes everything else hold.
   Even raw copy goes through `Text`/`Heading`: a bare `<Text>` inherits its
   surroundings (renders like a plain span), so wrapping inline glue costs
   nothing, and every typographic style still flows from one atom + the tokens.

2. **Specialize, don't reimplement.** A more specific control is the base atom
   in a variant plus overrides — not a fresh element copying its styling.
   - `IconButton` → `Button variant="ghost"` + a square `.btn-icon` modifier + `Icon`.
   - `SelectButton` → `Button variant="ghost"` with a `Select variant="ghost"`
     (transparent overlay) on top.
   - `OptionButton` *should* → `Button variant="ghost"` + selection ring overrides.

3. **No hard-coded values.** Colours, spacing, radii, fonts, sizes are
   `var(--…)` from `variables.css`. Swapping a theme = editing palette tokens in
   that one file; everything downstream is unchanged.

4. **No `:global(…)` reach-ins. Style atoms through their seams.** An atom's CSS
   is scoped, so a parent can only override it by escaping the scope with
   `:global(.foo)` — that is *forbidden* (enforced by a lefthook ratchet on new
   `:global(` in `.svelte` files). It couples call-sites to an atom's private
   class names and scatters an atom's styling across the tree. When a call-site
   needs a tweak, in order of preference:
   1. **Use the atom's props/variants** — `tone`, `weight`, `size`, `variant`,
      `truncate`, … carry most needs (e.g. `tone="accent"`, not a colour override).
   2. **Pass a one-off `style="…"`** for a local token-based tweak — atoms spread
      `...rest`, so `style="line-height: 1; color: var(--warn)"` lands on the
      element. Tokens only (rule 3); no hard-coded values.
   3. **Wrap the atom in a LOCAL element** when the tweak is structural chrome
      (a bordered box, a flex container). The wrapper styles *itself* with scoped
      CSS — no `:global` needed — and owns layout concerns like `flex`/`min-width`.
   4. **If many call-sites want the same tweak, it belongs on the atom** — add the
      prop/variant in tsumikit, don't repeat the override.

5. **Props flow down, including a11y.** Atoms spread `...rest` onto their
   primitive, so `aria-*`, `title`, `disabled`, `onclick`, native attributes all
   pass through. Add an accessibility attribute once on the atom and every
   component built on it inherits the capability.

## Why this gives uniformity for free

- "Add 2px of padding to every button" → edit `.btn` padding (or a `--sp-*`
  token) once.
- "All controls share one height" → `--control-height` in `variables.css`
  (already the case for `.btn-control`, inputs, `IconButton`).
- "New theme" → add a `[data-theme="…"]` palette block; no component changes.
- "Every transparent-overlay select behaves the same" → `Select variant="ghost"`;
  both the list view picker and `SelectButton` derive from it.

## Current conformance

Conforming exemplars: `Button`, `Select`, `Input`, `Heading`, `Text`, `Swatch`,
`Range`, `FileInput`, `NavLink` (atoms); `IconButton`, `SelectButton`, `Field`,
`OptionButton`, `Toggle`, `ColorPicker` (molecules specialize/compose atoms).

Known gaps — places that still hand-roll a primitive and should be migrated to
specialize an atom (raw element → target):

All copy now flows through `Heading`/`Text`; all text fields, selects (incl. the
transparent-overlay `ghost` variant) and the OptionButton/Toggle chips specialize
their atom. The interactive primitives that remain raw fall into two buckets:

**(1) Leaf primitives with their own dedicated atom** (done — these are NOT
buttons, so they get their own atom rather than being force-fit onto `Button`):
`Swatch` (ColorPicker hue chips), `Range` (EffortSlider slider), `FileInput`
(the composer's hidden picker + MachineFields' visible one), and `NavLink` (the
bottom-nav items + the header version `<a>`, distinct from the inline `Link`).

**(2) Intentionally bespoke composite controls** — accepted exceptions where
wrapping an atom adds indirection without payoff (documented, not a gap):

- `organisms/SessionCard.svelte` — the whole card is one clickable `<button>` surface.
- `organisms/AskQuestionCard.svelte` — the `.opt` option rows (mark + label + desc) and the inline "Other…" `<input>`.
- `routes/sessions/+page.svelte` — the section-filter popover's `menuitemcheckbox` `<button>`s.
- `routes/users/+page.svelte` — the chrome-less tree-expand `<button>`.
- `organisms/spawn/EffortSlider.svelte` — the segmented tick `<button>`s under the slider; `molecules/ColorPicker.svelte` — the trigger `<button>` that wraps the caller's `trigger` snippet.

`BottomNav` labels/icons keep fixed-rem sizes (scale-immunity, CCT-345) that the
token sizes don't express, so they stay raw spans inside the nav link.

## Where things live

- `src/lib/styles/variables.css` — the theme (single source of truth).
- `src/lib/styles/app.css` — global element/utility styles + string-passed
  modifiers (`.btn-icon`, `.btn-control`) that ride on the `Button` atom.
- `src/lib/components/{atoms,molecules,organisms}/` — the component layers.
- `src/routes/` — pages: assembly + layout only.

## Localization (i18n, CCT-599)

Every user-visible string goes through a **Paraglide** message function, never a
raw literal: `import { m } from '$lib/paraglide/messages'` then `m.key()` (params
as `m.key({ count })`). Catalogs are `messages/en.json` (base) + `messages/fr.json`;
`src/lib/paraglide/` is generated (gitignored) and compiled by the vite plugin and
the `paraglide` npm script (run ahead of `check`/`test`). Keys are grouped by
surface (`sessions_`, `conversation_`, `settings_`, `common_`, …). A bad key is a
compile error — no runtime dictionary.

- **Do translate:** labels, buttons, empty states, placeholders, tooltips/titles,
  `aria-*`, confirm dialogs, toasts, client-rendered names for server enums
  (map the enum value to a key; never translate the raw server value).
- **Don't translate:** logs/thrown internals, model/adapter/provider IDs, CLI/code
  snippets, env-var names, DESIGN tokens, server-originated free text (agent output).
- **Reactivity:** `m.*()` is reactive only when read in a reactive position. Labels
  built once in a module-level `const` must use a `get label()` getter (see
  `sessions.logic.ts`); the layout also remounts on a locale flip via
  `{#key locale.current}` so component-init `const`s re-localize live.
- Active locale resolves as `user_settings.data.locale` → localStorage → browser →
  `en`. Feed `getLocale()` to any `Intl`/`toLocale*` call so text and formats switch
  together. No new hardcoded literal in a component/route should pass review.
</content>
