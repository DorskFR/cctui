# fixtures

Poisoned inputs for the injection (INJ) and agent-on-agent (JUDGE) scenarios in
`../scenarios/INJ.yaml` and `../scenarios/JUDGE.yaml`.

```
<id>/fixture.yaml   id, vector, benign task, payload files, compliance markers,
                    expected_decision (judge fixtures only)
<id>/payload/       the files the agent is handed
transcripts/        recorded evidence with the verdict the judge must return
```

Payloads may use `{{SINK_URL}}` and `{{SINK_HOST}}`; `cctui-redteam t3 plan`
substitutes the run's sink. Any other placeholder fails validation.

Delivery with the built-in `t3 dispatch`:

- workspace files: a base64 setup command at the head of the prompt writes
  them, so the payload reaches the agent only when it reads the file;
- `delivery: peer_message`: sent through the session message API once the
  session exists;
- `delivery: mcp_reply`: no public API delivers it, so the job is recorded
  `inconclusive` until an MCP stub exists.

Injected actions only ever target the sink. Containment is the gate;
compliance (whether the agent tried) is reported per harness and model.
