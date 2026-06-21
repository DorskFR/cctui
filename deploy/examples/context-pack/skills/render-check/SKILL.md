---
name: render-check
description: Oracle skill for the `frontend` (and `brand-visible`) surface — exercise the changed UI in-pod with a headless browser and capture a screenshot/video of the actual rendered behaviour, so review is a glance at the pixels rather than a re-run. Run it in the implement step when the classification plan lists `render-check`; it produces the `screenshot`/`video` evidence the evidence-gate demands for `frontend`. Cannot judge taste — a `brand-visible` surface still routes its render to a human via the brand gate.
---

# render-check

The oracle for surfaces a human *sees*. A `frontend` change is verified by
**rendering it and looking** — the harness drives the changed UI in a headless
browser inside the pod, exercises the behaviour the acceptance condition names,
and captures the rendered result. The screenshot/video is the evidence: review
becomes a glance at the pixels, not a checkout-and-run.

Run this in the implement step (Step 3) when the classification plan lists
`render-check`. It needs the model network plus whatever local host the dev
server binds — never a third party.

## What it verifies

The acceptance condition, observably, in the rendered DOM:

1. **Boot the surface** — start the dev server / preview build in-pod, navigate
   the headless browser to the changed route/component.
2. **Drive the behaviour** — perform the interaction the acceptance condition
   names ("click checkout → total shows $42.00"), waiting on the real rendered
   state, not a fixed sleep.
3. **Capture** — screenshot the relevant region (a `video` when the condition is
   an interaction/animation), at a pinned viewport so the artifact is stable.

## Harness

A pack wires this to its real preview/build + browser-driver; the role is fixed:

```
<start dev server / preview build, bind localhost>
<headless browser: navigate → interact → screenshot/video>
```

- **Local network only.** `frontend` takes the model net plus the loopback dev
  server; it does **not** get a third-party `[network]` set. A frontend change
  that needs to call an external party also touches `external-api` — re-classify.
- **Pin the viewport + state.** A floating viewport or live data makes the
  screenshot un-reviewable; fix the size and seed deterministic data so the
  artifact shows the change, not the noise.

## Taste is routed, not checked

`render-check` proves the UI **renders and behaves** as the acceptance condition
says. It does **not** — cannot — judge whether the copy is on-brand, the layout
tasteful, or the price right. When the surface is `brand-visible`, the classifier
sets `brand_gate: true`: the captured screenshot is additionally routed to a
human taste sign-off via `needs_human`, **independent** of the merge decision.
The oracle's job is to make that sign-off a glance at a faithful render.

## Evidence it produces

Feeds the `evidence-gate` for the `frontend` / `brand-visible` surface —
`screenshot` (or `video`) + `diff`:

```yaml
evidence:
  - kind: screenshot
    surface: frontend
    summary: <e.g. "checkout renders the new total $42.00">
    detail: <image URL / artifact path>
  - kind: diff
    surface: frontend
    summary: <one line>
    detail: |
      <unified diff / key hunks>
```
