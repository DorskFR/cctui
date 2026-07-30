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
                  No component hard-codes a hex or a pixel. The token NAMES are
                  the contract Tsumikit styles itself from.
        ↓
Tsumikit        → @dorsk/tsumikit: the external design system, and the ONLY
(the atoms)       place a raw HTML primitive (<button>, <select>, <input>,
                  <textarea>, <a>, <h1>-<h6>, and text-bearing
                  <p>/<span>/<label>) comes from. Owns its own scoped styling
                  and exposes base shapes + props/variants:
                  Button, Select, Input, Textarea, Switch, Icon, IconButton,
                  Card, Field, Modal, Badge, Link, Popover, Tooltip, Progress,
                  Toggle, OptionButton, SelectButton, FileButton, Dropzone,
                  FilterSearchBar, Timestamp, layout (Stack, Cluster, AutoGrid,
                  Container, Tabs), and Heading / Text for ALL copy.
        ↓
Local atoms     → lib/components/atoms/*: ONLY domain-specific leaves Tsumikit
                  has no concept of — AdapterIcon, BrandLogo, Error, NavLink,
                  Range, Swatch. Not a parallel design system: never re-add a
                  Button/Select/Input/Text/ColorPicker here.
        ↓
Molecules       → lib/components/molecules/*: combine Tsumikit components, or
                  SPECIALIZE one via a variant + overrides. Never reimplement a
                  primitive. e.g. SessionDot, TokenUsage, LabelBadge, SoftLimit.
        ↓
Organisms       → lib/components/organisms/*: purpose-driven assemblies of
                  molecules + atoms. e.g. ConversationDrawer, SpawnModal,
                  SessionCard, Header.
        ↓
Pages / routes  → routes/*: assemble organisms, .ts logic (queries, format,
                  drafts) and layout. Presentation only — no bespoke controls.
```

Generic controls are **upstream work**: a missing prop, variant or component
belongs in Tsumikit, not in a local re-implementation. Only genuinely
app-specific concerns live in this repo.

## Rules

1. **Primitives come from Tsumikit.** If you're typing `<button>`, `<select>`,
   `<input>`, `<textarea>`, `<a>`, an `<h1>`–`<h6>`, or a text-bearing
   `<p>`/`<span>`/`<label>`, stop — import the Tsumikit component (or add a
   prop/variant upstream). This is the rule that makes everything else hold.
   Even raw copy goes through `Text`/`Heading`: a bare `<Text>` inherits its
   surroundings (renders like a plain span), so wrapping inline glue costs
   nothing, and every typographic style still flows from one component + the tokens.

2. **Specialize, don't reimplement.** A more specific control is a Tsumikit
   component in a variant plus overrides — not a fresh element copying its
   styling. `IconButton`, `SelectButton`, `OptionButton`, `Field` and `Toggle`
   already exist upstream as exactly these specializations; reach for them
   before writing a new control. A local wrapper is justified only when it adds
   *domain* behaviour, not styling.

3. **No hard-coded values.** Colours, spacing, radii, fonts, sizes are
   `var(--…)` from `variables.css`. Swapping a theme = editing palette tokens in
   that one file; everything downstream is unchanged.

4. **No `:global(…)` reach-ins. Style Tsumikit components through their seams.**
   A `class="…"` you pass to a Tsumikit component lands on *its* root element,
   which your scoped CSS cannot reach — so the only way to style it is to escape
   the scope. That is *forbidden* for new code (enforced by a ratchet on added
   `:global(` lines in `.svelte` files): it couples call-sites to private class
   names and scatters one component's styling across the tree. When a call-site
   needs a tweak, in order of preference:
   1. **Use the component's props/variants** — `tone`, `weight`, `size`, `variant`,
      `truncate`, … carry most needs (e.g. `tone="accent"`, not a colour override).
   2. **Pass a one-off `style="…"`** for a local token-based tweak — Tsumikit
      spreads `...rest`, so `style="line-height: 1; color: var(--warn)"` lands on
      the element. Tokens only (rule 3); no hard-coded values.
   3. **Wrap it in a LOCAL element** when the tweak is structural chrome (a
      bordered box, a flex container). The wrapper styles *itself* with scoped
      CSS — no `:global` needed — and owns layout concerns like `flex`/`min-width`.
   4. **If many call-sites want the same tweak, it belongs upstream** — add the
      prop/variant in Tsumikit, don't repeat the override.

   **An unscoped `:global(.foo)` is never acceptable.** Svelte emits it verbatim
   into the app-wide stylesheet, so it silently restyles every `.foo` in every
   route. Generic names (`.page-title`, `.bar`, `.grow`, `.secret`, `.count`,
   `.name`) *will* collide — two routes each declaring `:global(.page-title)`
   union their rules onto every heading in the app, which is exactly the bug this
   rule exists to prevent. If escaping the scope is genuinely unavoidable, it must
   be **anchored to a local ancestor** so it cannot leak:

   ```css
   /* NO — leaks app-wide */
   :global(.page-title) { font-size: 28px; }

   /* YES — the leftmost selector is this component's own element, so the
      override reaches only descendants of THIS component's DOM */
   .bar > :global(.sess-title) { font-size: 28px; }
   ```

   Note `:global()` may only sit at the **start or end** of a selector sequence,
   never in the middle (`.a :global(.b) .c` is a compile error) — so anchor with
   `.local :global(.theirs)` and, if you also need a deeper element of your own,
   select it directly (`.local .mine`) since your own elements carry the scope hash.

   Styles for `{@html}`-rendered markup (markdown, highlighted code) are the one
   legitimately global case: those elements exist in no component's template, so
   they belong in `app.css` under a namespaced parent selector, not in a
   component's `:global()`.

5. **Props flow down, including a11y.** Tsumikit components spread `...rest` onto
   their primitive, so `aria-*`, `title`, `disabled`, `onclick`, native attributes
   all pass through. Add an accessibility attribute once upstream and every
   component built on it inherits the capability.

## Why this gives uniformity for free

- "New theme" → add a `[data-theme="…"]` palette block in `variables.css`; no
  component changes, and Tsumikit re-skins with it because it styles from the
  same token names.
- "All controls share one height" → `--control-height`; every control inherits it.
- "Add 2px of padding to every button" → a token change here, or a one-line
  Tsumikit release — never 50 hand-edits.
- "Every transparent-overlay select behaves the same" → `Select variant="ghost"`;
  both the list view picker and `SelectButton` derive from it.

## Current conformance

`Button`, `Select`, `Input`, `Textarea`, `Heading`, `Text`, `Card`, `Field`,
`Modal`, `IconButton`, `SelectButton`, `OptionButton`, `Toggle`, `ColorPicker`,
`FilterSearchBar` and the layout primitives all come from `@dorsk/tsumikit`
(~70 import sites). They used to live in this repo; do not re-add local copies.

Known gaps — places that still hand-roll a primitive and should be migrated to
specialize a Tsumikit component (raw element → target):

All copy now flows through `Heading`/`Text`; all text fields, selects (incl. the
transparent-overlay `ghost` variant) and the OptionButton/Toggle chips specialize
their upstream component. The interactive primitives that remain raw fall into
two buckets:

**(1) Domain leaves with their own local atom** (done — these are NOT buttons, so
they get a dedicated atom rather than being force-fit onto `Button`): `Swatch`
(hue chips), `Range` (EffortSlider slider), `NavLink` (the bottom-nav items + the
header version `<a>`, distinct from Tsumikit's inline `Link`), plus `AdapterIcon`,
`BrandLogo` and `Error`.

**(2) Intentionally bespoke composite controls** — accepted exceptions where
wrapping an atom adds indirection without payoff (documented, not a gap):

- `organisms/SessionCard.svelte` — the whole card is one clickable `<button>` surface.
- `organisms/AskQuestionCard.svelte` — the `.opt` option rows (mark + label + desc) and the inline "Other…" `<input>`.
- `routes/sessions/+page.svelte` — the section-filter popover's `menuitemcheckbox` `<button>`s.
- `routes/users/+page.svelte` — the chrome-less tree-expand `<button>`.
- `organisms/spawn/EffortSlider.svelte` — the segmented tick `<button>`s under the slider; `molecules/ColorPicker.svelte` — the trigger `<button>` that wraps the caller's `trigger` snippet.

`BottomNav` labels/icons keep fixed-rem sizes (scale-immunity, CCT-345) that the
token sizes don't express, so they stay raw spans inside the nav link.

## File size

Per the repo-root [DESIGN.md](../DESIGN.md): prefer **reasonable file lengths
(< ~300 LOC)**. Past that, split — extract a molecule, or break an organism into
sub-components. The same doc covers the dependency policy (prefer Tsumikit or the
platform over a new package; no CDNs).

## Where things live

- `src/lib/styles/variables.css` — the theme (single source of truth), including
  the token names Tsumikit consumes.
- `src/lib/styles/app.css` — global element/utility styles, `{@html}`-markdown
  styling under a namespaced parent, + string-passed modifiers (`.btn-icon`,
  `.btn-control`) that ride on the `Button` component.
- `src/lib/components/atoms/` — domain-specific leaves only (see the layers above).
- `src/lib/components/{molecules,organisms}/` — the composed layers.
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
