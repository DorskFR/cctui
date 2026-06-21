---
name: prior-art
description: Before introducing any new utility, type, component, or helper in the implement step, search the repo's generated helper index for an existing one and CITE the prior art you will reuse — or state why nothing fits and a new one is warranted. This is the up-front, retrieval half of the consistency problem: the agent reinvents because it does not know the helper exists. Run as a required sub-step of implement, before the first new abstraction is written.
---

# prior-art

The dual of the `consistency-gates` ratchet. Duplication and reinvention are
~80% **human-caught** at review — but the gate that catches a *paste* after the
fact (jscpd) cannot catch a *reinvention*: the agent wrote a new `formatMoney`
from scratch because it never knew `domain/format.ts#formatMoney` existed. That
is a **retrieval** failure, not a verification one. The fix is to make the
existing surface discoverable and to require the agent to *look before it adds*.

## The helper index

A pack ships (or generates in CI) a **helper/utility index** at a known path
(e.g. `/opt/context/docs/helper-index.md`) — a flat, searchable list of the
repo's reusable surface: shared utilities, domain types, UI components, hooks,
and the modules they live in, each with a one-line "use this when…". It is
generated from the codebase (a doc-comment / export scan), not hand-maintained,
so it does not drift. The index is what makes "we already have this" a
**lookup** instead of tribal knowledge.

```
# regenerate before a run so the index reflects current HEAD
make helper-index    # scans exports + doc-comments -> docs/helper-index.md
```

## The cite-prior-art step

Before you introduce the **first new** utility, type, component, or helper in the
implement step, run this check:

1. **Search** the helper index (and the codebase) for the capability you are
   about to add — by name, by signature, by what it does.
2. **Cite** the prior art you will reuse: name the existing util/type/component
   and the module it lives in, and use it.
3. **Or justify** a new one: if nothing fits, state in one line *why* (the
   nearest existing helper and how it falls short), then add it — and consider
   whether it belongs in the index for the next agent.

Express this as a required gate inside the implement step: a code change that
introduces a new abstraction without a `prior_art` citation (a reused reference
or a justified-new line) is incomplete. It is the inverse of the jscpd gate —
jscpd catches the paste at the end; this catches the reinvention at the start,
which is cheaper and leaves a citation in the record.

## Output shape

Emit a terse block the evidence/diff carries, one entry per new abstraction the
change introduces:

```yaml
prior_art:
  - needed: <capability, e.g. "format a money amount for display">
    found: <module#symbol reused>      # e.g. domain/format.ts#formatMoney
    # OR, when nothing fit:
    # found: none
    # justified: <nearest existing helper + why it does not fit>
```

An empty `prior_art` block is valid only for a change that introduces **no** new
abstraction (a pure edit to existing code). Any change that adds a util/type/
component carries at least one entry — a reuse citation or a justified-new line.

## Why it composes

`prior-art` (retrieval, up front) and `consistency-gates`/jscpd (detection, at
the end) bracket the reinvention problem from both sides: the citation makes the
agent reuse before it writes, and the gate catches it if it pasted anyway. The
helper index is the shared substrate — the same surface a human scans before
asking "didn't we already build this?", now in the agent's context.
