---
name: roundtrip-check
description: Oracle skill for the `external-api` (and the outbound half of `payments`) surface — exercise a third-party integration in-pod against test-mode / recorded cassettes (VCR-style record-replay) plus contract tests, and capture the request/response transcript as evidence. Run it in the implement step when the classification plan lists `roundtrip-check`; it produces the `transcript` evidence the evidence-gate demands for `external-api`/`payments`. Always `human-gate` — proves the round-trip succeeds, not that calling the third party is wanted.
---

# roundtrip-check

The oracle for surfaces that **leave the building**. An `external-api` change
calls a third party, so it cannot be verified offline and must not hammer the
real production API. The harness exercises the integration in-pod against a
**test-mode endpoint or a recorded cassette** (VCR-style record-replay) and
captures the actual request/response **transcript** as evidence: the round-trip
demonstrably succeeds, with the wire traffic attached for review.

Run this in the implement step (Step 3) when the classification plan lists
`roundtrip-check`. The outbound (charge/refund) half of a `payments` change uses
this oracle too, paired with `contract-check` for the inbound webhook.

## What it verifies

The integration's real round-trip, reproducibly:

1. **Record once, replay after** — on first authoring, record the third party's
   responses to a **cassette** (test-mode credentials, never prod). Subsequent
   runs **replay** the cassette: deterministic, offline, no rate limits, no live
   side effects.
2. **Contract tests** — assert the request the change *sends* matches the
   provider's contract (required fields, auth header shape, idempotency key) and
   the response is parsed correctly — so a provider schema drift is caught.
3. **Test-mode live, where it exists** — for providers with a sandbox (e.g.
   Stripe test mode), exercise the live sandbox once and snapshot it to the
   cassette; thereafter replay.

## Harness

A pack wires this to its real cassette library + the provider's sandbox; the
role is fixed:

```
<replay cassette (or record once vs test-mode sandbox)>
<contract test: assert request shape + parsed response against the provider contract>
```

- **Test-mode / replay only — never prod.** The `[network]` set granted is the
  provider's **sandbox** host, not its production host. A run that needs the
  production endpoint is mis-scoped; stop.
- **Cassettes are reviewed code.** A re-recorded cassette is a diff: a provider
  schema change shows up there, and an unexplained cassette churn is a red flag.
- **Always `human-gate`.** Per the classifier, `external-api` and `payments`
  never auto-merge: the oracle proves the call *works*, not that making it is
  *wanted*. A human signs off the merge.

## Evidence it produces

Feeds the `evidence-gate` for `external-api` / `payments` — `transcript` +
`diff`:

```yaml
evidence:
  - kind: transcript
    surface: external-api
    summary: <e.g. "test-mode charge succeeds: POST /charges → 200, id ch_test_…">
    detail: |
      → POST https://sandbox.example.com/charges  {…request…}
      ← 200  {…response…}
  - kind: diff
    surface: external-api
    summary: <one line>
    detail: |
      <unified diff / key hunks>
```
