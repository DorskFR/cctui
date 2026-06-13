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

4. **Override by specificity, never by forking.** When a call-site needs a tweak
   on an atom, pass `class="…"` and target it with higher specificity
   (e.g. `:global(.input.search)` beats the atom's `.input` base regardless of
   bundle order). The atom keeps owning the base; the tweak is additive and
   local. Don't copy the atom's CSS to change one value.

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
</content>
