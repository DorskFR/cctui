---
name: endpoint-tests
description: Oracle skill for the `backend` surface — exercise the changed server logic (API route, job, query) in-pod against a test instance and assert the observable response/effect, so review is a passing run rather than reading the handler. Run it in the implement step when the classification plan lists `endpoint-tests`; it produces the `test-run` evidence the evidence-gate demands for `backend`. The route returns 200 with field Z / the job writes row R — proven, not asserted.
---

# endpoint-tests

The oracle for server behaviour. A `backend` change — an API route, a background
job, a query — is verified by **exercising it against a test instance in-pod**
and asserting the observable effect: the endpoint returns the expected status +
body, the job produces the expected row, the query returns the expected set. The
passing run is the evidence; review is reading the assertions, not re-deriving
the handler.

Run this in the implement step (Step 3) when the classification plan lists
`endpoint-tests`. It needs the model network plus the loopback test server/db —
no third party.

## What it verifies

The acceptance condition as an observable server effect:

1. **Stand up the surface** — boot the server / migrate a throwaway test db /
   enqueue against a test worker, in-pod, seeded with deterministic fixtures.
2. **Exercise it** — issue the request / trigger the job the acceptance
   condition names.
3. **Assert the effect** — status + response shape for a route; the written row
   / emitted event / returned set for a job or query. Assert the **observable
   outcome**, not internal calls.

## Harness

A pack wires this to its real server + test-db harness; the role is fixed:

```
<migrate throwaway test db, boot server / worker on localhost>
<integration test: request/trigger → assert status + body / effect>
```

- **Local network only.** `backend` gets the model net plus the loopback test
  instance; it does **not** get a third-party `[network]` set. A handler that
  calls an external party also touches `external-api`/`webhook` — re-classify and
  add the matching oracle (`roundtrip-check` / `contract-check`).
- **Throwaway state.** Each run migrates a fresh test db and tears it down — a
  test that depends on leftover state is not reproducible in-pod.
- **Migrations are human-gate.** A change that alters the schema is always
  `human-gate` per the classifier; `endpoint-tests` proves the new shape works
  but never auto-merges it.

## Evidence it produces

Feeds the `evidence-gate` for the `backend` surface — `test-run` + `diff`:

```yaml
evidence:
  - kind: test-run
    surface: backend
    summary: <e.g. "POST /orders returns 201 with id; row persisted">
    detail: |
      $ <test-runner> <backend target>
      <output, exit status 0 visible>
  - kind: diff
    surface: backend
    summary: <one line>
    detail: |
      <unified diff / key hunks>
```
