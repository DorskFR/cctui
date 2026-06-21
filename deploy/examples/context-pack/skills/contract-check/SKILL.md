---
name: contract-check
description: Oracle skill for the `webhook` (and the inbound half of `payments`) surface — replay recorded inbound payload fixtures against the changed handler in-pod, assert the contract (signature verification, idempotency, the response/effect), and capture the request/response transcript as evidence. Run it in the implement step when the classification plan lists `contract-check`; it produces the `transcript` evidence the evidence-gate demands for `webhook`/`payments`. Always `human-gate`.
---

# contract-check

The oracle for the **inbound** contract — a webhook handler, or the
event-receiving half of a payments integration. The third party POSTs a payload;
correctness is whether the handler honours the **contract**: verifies the
signature, is idempotent on redelivery, and produces the right response + effect.
The harness **replays recorded payload fixtures** against the changed handler
in-pod and captures the request/response **transcript** as evidence.

Run this in the implement step (Step 3) when the classification plan lists
`contract-check`. The inbound (event-received) half of a `payments` change uses
this oracle, paired with `roundtrip-check` for the outbound charge/refund.

## What it verifies

The handler against the provider's inbound contract, reproducibly:

1. **Replay fixtures** — recorded real payloads (captured once from the
   provider's test-mode / a `stripe trigger`-style replay), POSTed at the local
   handler. No live inbound connection; the fixtures are checked in.
2. **Signature verification** — a payload with a valid test signature is
   accepted, a tampered/absent one is rejected. This is the security-critical
   half — assert both directions.
3. **Idempotency** — the same delivery replayed twice produces the effect
   **once** (providers retry; a non-idempotent handler double-applies).
4. **Response + effect** — the handler returns the status the provider expects
   (so it stops retrying) and the intended state change happened.

## Harness

A pack wires this to its real fixture set + local replay; the role is fixed:

```
<replay recorded payload fixtures (valid + tampered + duplicate) at localhost handler>
<assert: signature accept/reject, single effect on redelivery, expected response>
```

- **Replay only — no live inbound.** The handler is exercised against checked-in
  fixtures on loopback; no public ingress, no provider callback. The `[network]`
  granted is the model net plus loopback, not a third party.
- **Fixtures are reviewed code.** A re-captured payload is a diff; an unexplained
  fixture change is a red flag.
- **Always `human-gate`.** Per the classifier, `webhook` and `payments` never
  auto-merge — the oracle proves the contract is honoured, a human signs off the
  merge.

## Evidence it produces

Feeds the `evidence-gate` for `webhook` / `payments` — `transcript` + `diff`:

```yaml
evidence:
  - kind: transcript
    surface: webhook
    summary: <e.g. "checkout.session.completed: valid sig → 200, effect once on redelivery; tampered → 400">
    detail: |
      → POST /webhooks/stripe  (valid test sig)   ← 200  (order marked paid)
      → POST /webhooks/stripe  (same delivery, retry) ← 200  (no double-apply)
      → POST /webhooks/stripe  (tampered sig)     ← 400
  - kind: diff
    surface: webhook
    summary: <one line>
    detail: |
      <unified diff / key hunks>
```
