---
name: consistency-gates
description: Run the repo's deterministic consistency gates — structural lint (ast-grep / Semgrep), layering boundaries (dependency-cruiser), and copy-paste detection (jscpd) — in the implement step, read their output, fix the violations, and codify a NEW rule when a recurring violation surfaces (the ratchet). These gates catch the class of defects oracles and review miss: code that is *correct* but *inconsistent* (banned API, broken layer boundary, duplicated logic). Run when the classification plan lists `consistency-gates`; the `[gate]` on the implement step blocks the transition until they exit green.
---

# consistency-gates

The oracles prove the change *works*; the evidence gate proves it is *finalized
on proof, not assertion*. Neither catches the change that is correct but
**inconsistent** — it uses a banned API, crosses a layering boundary, or
re-implements logic that already exists three modules over. That class of defect
is empirically ~80% **human-caught** at review (the Acme review-mining
finding): a bot passes correctness but is blind to "we already have this" and
"we don't do it that way here". Those review comments are not verification
failures — they are *codification* failures: the convention lived in a
reviewer's head instead of in a rule the CI runs.

This skill closes that gap by moving the convention into **deterministic gates
that live in the repo**, so they protect humans too, and run identically in the
worker. Run it in the implement step (Step 3) when the classification plan lists
`consistency-gates` among its gates.

## The three gates

A pack wires each to its real toolchain; the role of each is fixed. All run
locally — no network, no secrets.

1. **Structural lint — `ast-grep` (or Semgrep).** Pattern rules over the AST,
   not text: banned APIs (`console.log`, `unwrap()` in a lib, a deprecated
   client), required house patterns (every handler wraps errors in `AppError`,
   every query goes through the repository layer). Each rule is a few lines of
   YAML; a violation points at the exact node.
2. **Layering boundaries — `dependency-cruiser`.** The allowed import graph:
   `ui → domain → data`, never the reverse; no module reaches across a feature
   boundary; no test util is imported by production code. A new edge that
   violates the declared boundary fails the gate with the offending import.
3. **Copy-paste — `jscpd`.** Token-level duplication detection over the diff.
   A block pasted from elsewhere (the most common "you already have this") trips
   it; the report names both locations so the fix is to extract or reuse, not to
   re-paste.

## Harness

```
# run all three; exit non-zero on any violation, print the offending location
ast-grep scan            # structural rules in sgconfig.yml
depcruise --validate     # layering rules in .dependency-cruiser.js
jscpd --threshold 0      # copy-paste over the changed paths
```

A pack collapses these behind one command (e.g. `make consistency-check`) and
points the implement step's `[gate]` at it, so the transition into the finalize
step refuses to fire until the gates are green — the same structural enforcement
the oracles get.

## Read the output, then fix — don't suppress

A violation is a signal, not an obstacle:

- **Banned API / wrong pattern** → use the house pattern the rule points to.
- **Layering violation** → the dependency is in the wrong direction; move the
  code or invert the dependency, do not add an inline-ignore.
- **Duplication** → reuse or extract the existing block (this is the same
  finding the prior-art step is meant to prevent up front — see the
  `prior-art` skill). Re-pasting and suppressing jscpd defeats the gate.

Suppressing a rule inline is a `judgment` decision, not a mechanical one: it
needs a one-line reason and, on a non-low-blast surface, a human (route via
`needs_human`). A silent `// eslint-disable` / `# ast-grep-ignore` is a red flag
in the `diff` evidence.

## The ratchet — codify a NEW rule

This is the half that makes the gate *grow*. When you hit a violation that is
**recurring** — the same review comment you have seen before, or a pattern a
human would flag — do not just fix this instance: **add the rule** so the next
agent (and the next human) cannot reintroduce it. A new mechanical convention
becomes a **one-rule PR**, not a recurring review comment.

The ratchet test: *would a reviewer write this same comment on the next PR?* If
yes and the rule is mechanically checkable, add an `ast-grep` / dep-cruiser /
jscpd rule in the same change. If it is a taste/judgment convention that cannot
be expressed mechanically, it does not belong in a gate — leave it to review.

## Evidence it produces

Feeds the `evidence-gate` as a `test-run` for whichever surfaces the change
touched (consistency gates are surface-agnostic — they run on every code-touching
flow):

```yaml
evidence:
  - kind: test-run
    surface: <touched surface>
    summary: <e.g. "ast-grep + depcruise + jscpd clean; added 1 banned-API rule">
    detail: |
      $ make consistency-check
      <output, exit status 0 visible>
  - kind: diff
    surface: <touched surface>
    summary: <one line, incl. any NEW rule added by the ratchet>
    detail: |
      <unified diff / key hunks, incl. the rule file change>
```

A green consistency run plus, where it applies, a one-rule ratchet commit is the
mechanical convention turned into code — the recurring review comment retired at
its source.
