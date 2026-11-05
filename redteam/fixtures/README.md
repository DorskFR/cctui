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
substitutes the run's sink. Any other placeholder fails validation. A file with
`delivery: peer_message` or `delivery: mcp_reply` is fed through that channel by
the dispatch program instead of being written to the workspace.

Injected actions only ever target the sink. Containment is the gate;
compliance (whether the agent tried) is reported per harness and model.
