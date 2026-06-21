---
name: visual-evidence
description: The concrete in-pod harness for the `frontend` surface's `render-check` oracle — boot the changed UI against record/replay HTTP fixtures (no backend), drive the acceptance behaviour with a headless browser, and capture element-clip screenshots + a `toHaveScreenshot` before/after diff + a computed-style snapshot as the evidence. Run it in the implement step (Step 3) when the classification plan lists `render-check`; it produces the `screenshot`/`diff` evidence the evidence-gate demands for `frontend`. Generalizes the local manual visual loop into a deterministic, low-flake gate that needs no live backend.
---

# visual-evidence

`render-check` says *what* a `frontend` change is verified by — rendering it and
looking. **`visual-evidence` is the harness that does it**, in the pod, with no
live backend. It turns the manual "run vite, point a browser at prod, eyeball
it" loop into a deterministic gate: mock the data at the HTTP boundary, render
the real route, clip the changed element, and diff the pixels against a checked-in
baseline. The screenshot + diff is the evidence the `evidence-gate` collects for
the `frontend` surface; the computed-style snapshot is the low-flake assertion
that makes a green run trustworthy.

Run this in the implement step (Step 3) when the classification plan lists
`render-check`. It needs the model network plus the loopback dev/preview server
only — never a third party. (A `frontend` change that must call an external host
also touches `external-api` — re-classify rather than widening this set.)

## Why mock at the HTTP boundary

The whole cost of frontend verification is the stack behind it: backend, DB, CMS,
auth. Booting that in-pod is slow, flaky, and often impossible. So the data is
**recorded once and replayed** — the UI renders against a frozen fixture, so the
screenshot shows the change, not yesterday's live data.

- **Client-fetched data (SPA / CSR)** — record the network once from a real
  origin into a HAR, then replay it. With Playwright:

  ```ts
  // record (once, against a real origin — produces the checked-in fixture):
  await context.routeFromHAR('fixtures/sessions.har', { url: '**/api/**', update: true });
  // replay (in-pod, the gate): serve the recorded responses, fail on a miss:
  await context.routeFromHAR('fixtures/sessions.har', { url: '**/api/**', update: false, notFound: 'abort' });
  ```

  `update: false` + `notFound: 'abort'` is the contract: the run uses *only*
  recorded responses; an un-recorded request aborts rather than escaping to the
  network, so the gate stays hermetic.

- **Server-rendered data (SSR / RSC / SSG)** — a HAR at the browser boundary is
  too late; the fetch is server-side. Either snapshot the **static build**
  (`next build && next export`, or the framework's prerender) and serve the
  emitted HTML, or stub the server `fetch`/data-loader with a recorded fixture.
  Same principle: freeze the data, render the real component tree.

## The three captures

A pack wires this to its real preview build + browser driver; the role is fixed.
Pin the viewport and seed deterministic UI state (most state in an SPA is
`localStorage`-backed — set the keys before page scripts run) so the artifact
shows the change, not the noise.

1. **Element-clip screenshot** — screenshot the *changed element on the real
   route*, not a detached clone, so it inherits the route's contextual CSS
   (cascade, media queries, theme vars). `locator.screenshot()` clips to the
   element's box:

   ```ts
   await page.goto('http://localhost:5273/sessions');           // the real route
   await expect(page.getByTestId('session-card')).toHaveScreenshot('session-card.png');
   ```

2. **`toHaveScreenshot` new-vs-previous diff** — the assertion writes the
   baseline on first run and diffs against it after; a pixel delta over the
   threshold fails and emits the diff image. When the change is *intended*, the
   baseline is regenerated **in the same PR** so the review sees before→after,
   never a silently-updated baseline. An interaction/animation is captured as a
   `video` instead of a still.

3. **Computed-style snapshot** — the deterministic, low-flake gate. Anti-aliasing
   and font hinting make pure-pixel diffs flaky; assert the *resolved* styles of
   the elements the change touched so a layout regression fails sharply:

   ```ts
   const box = await page.getByTestId('session-card').evaluate(el => {
     const s = getComputedStyle(el);
     return { height: s.height, display: s.display, gridTemplateColumns: s.gridTemplateColumns };
   });
   expect(box).toMatchSnapshot('session-card.style.json');
   ```

   This is how layout root-causes get pinned (e.g. a row stuck at one height
   because a media-query override lost on source-order specificity) — far faster
   and far less flaky than guessing from a pixel diff alone.

## Harness shape

```
<build/serve the surface against the replay fixture, bind localhost>
<headless browser: seed state → navigate the real route → drive the behaviour>
<capture: element-clip screenshot + toHaveScreenshot diff + computed-style snapshot>
```

- **Local network only.** `frontend` gets the model net plus the loopback
  preview server — no third-party `[network]` set. The replay fixture is what
  removes the need for the backend; a run that reaches the network was either
  mis-recorded (a HAR miss) or mis-classified.
- **Determinism is the contract.** Pin the viewport (desktop + mobile if the
  change is responsive), `deviceScaleFactor`, the fixture, and the seeded state.
  Wait on the rendered state, never a fixed sleep. A flaky screenshot means a
  non-deterministic input (live data, animation, font load) — fix the input,
  don't retry until green.
- **Capture the live on-screen node.** A blank screenshot means the captured
  node is hidden/detached (`opacity:0`, unresolved CSS vars) — clip the rendered
  element, not a clone.

## Taste is routed, not checked

`visual-evidence` proves the UI **renders and behaves** as the acceptance
condition says. It does not — cannot — judge whether the copy is on-brand or the
layout tasteful. When the surface is `brand-visible` the classifier sets
`brand_gate: true`: the captured screenshot is additionally routed to a human
taste sign-off via `needs_human`, **independent** of the merge decision. The
oracle's job is to make that sign-off a glance at a faithful render. Figma is a
*start* reference for that human, not a parity oracle here — there is no
design-system diff yet.

## Evidence it produces

Feeds the `evidence-gate` for the `frontend` / `brand-visible` surface —
`screenshot` (or `video`) + `diff`:

```yaml
evidence:
  - kind: screenshot
    surface: frontend
    summary: <e.g. "session card renders the new compact density at 1280x900">
    detail: <image URL / artifact path — plus the toHaveScreenshot diff when a baseline existed>
  - kind: diff
    surface: frontend
    summary: <one line: what changed, incl. any regenerated screenshot/style baseline>
    detail: |
      <unified diff / key hunks>
```

A regenerated baseline (screenshot or computed-style snapshot) is a code change:
it appears in the `diff` evidence, and unexplained baseline churn is a red flag,
not a pass.
