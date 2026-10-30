# Regression eval

Fail and regression rates of the changes on `main`, grouped by the agent that
wrote them. Reads git history only: no database, no network, no API spend.

```sh
make eval/regressions                      # markdown tables on stdout
python3 evals/regressions/regressions.py --window-days 7 --json tmp/changes.json
make eval/regressions/test
```

## What it measures

A **change** is one first-parent commit of `main`: a squashed PR, a merged PR
with every commit it brought in, or a direct push. Its **agent** is the model
named most often in its `Co-Authored-By: Claude …` trailers; changes without one
are `human`, or `dependabot`.

| Column | Meaning |
|---|---|
| reverted % | a later commit reverts it (`This reverts commit <sha>`, or `Revert "<title>"`) |
| regressed % | a later `fix` change, within the window, modifies or deletes lines `git blame -w` attributes to it |
| fix share % | share of the group's changes that are fixes, i.e. its task mix |
| fixed lines/kLOC | lines later fixed per 1000 lines the group added |
| median days to fix | time from the change to the first fix that touched it |

Changes younger than the window are left out of the rates, since their fixes
may not have landed yet. Lockfiles, changelogs, generated schemas and
`version = …` lines are ignored, so release bumps are not counted as fixes.

## Reading it

- Compare agents on **Features only**: the overall table mixes task types, and
  an agent that mostly fixes bugs looks safer than one that ships features.
- `regressed %` is an upper bound: a fix that rewrites a line for an unrelated
  reason still counts. `fixed lines/kLOC` weighs that by size.
- A fix that only adds lines blames nothing, so missing guards are undercounted.
- `human` includes agent work committed without a trailer.
- Model generations overlap in time with different kinds of work; small groups
  (under ~30 changes) are noise.
