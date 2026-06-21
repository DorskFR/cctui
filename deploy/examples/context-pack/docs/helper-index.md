# Helper / utility index (context pack example)

The discoverability substrate for the `prior-art` step: a flat, searchable list
of the repo's **reusable surface** — shared utilities, domain types, UI
components, hooks — each with the module it lives in and a one-line "use this
when…". The agent searches this index before introducing a new abstraction and
**cites** the prior art it will reuse (or justifies a new one).

This fixture is NEUTRAL — the entries are placeholders. A **real** index is
**generated in CI** from the codebase (an export + doc-comment scan, e.g.
`make helper-index`), never hand-maintained, so it cannot drift from HEAD.

Format: `module#symbol — use this when …`

## Utilities

- `domain/format.ts#formatMoney` — render a money amount for display (locale + currency).
- `domain/format.ts#parseMoney` — parse a user-entered amount string to minor units.
- `lib/result.ts#tryAsync` — wrap a fallible async call into a `Result`, no throw.
- `lib/time.ts#toIsoDay` — normalize a timestamp to a UTC `YYYY-MM-DD` key.

## Domain types

- `domain/account.ts#Account` — the canonical account record (id, status, limits).
- `domain/money.ts#Money` — minor-units amount + currency; never use a bare `number`.

## UI components

- `ui/components/Button.tsx#Button` — the house button (variants, loading state).
- `ui/components/Field.tsx#Field` — labelled form control with error slot.

## Hooks

- `ui/hooks/useAsync.ts#useAsync` — run + track a fallible async call in a component.
