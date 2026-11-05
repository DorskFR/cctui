# red-team profiles and identity

Example worker profiles for red-team cells, and how to set up the identity T2
dispatches as. Every value here is a placeholder: the live deployment is
described only by the operator's T2 config (`redteam/ci/t2.example.json`) and
the workflow's secrets.

## The red-team identity

No schema: the identity is an ordinary cctui user the operator enrols.

1. Create a non-admin user (the config's `identity`, e.g. `redteam`) with no
   machines and no real sessions.
2. Issue it an API key with dispatch scope. That key is the
   `REDTEAM_T2_TOKEN` secret. `run-t2.sh` refuses to run when the key's user
   is not `identity`, is an admin, or is a machine.
3. Give it dedicated dispatchers and worker profiles (the config's
   `dispatchers` and `profiles`), placed where production workers run, under
   the same network policy, admission and base image.
4. Give those profiles their own service identity whose secret-backend role
   reads only canary secrets, and point every proxy credential rule they use at
   a canary value. Real credentials must be unreachable from the cell.

## The probe image

`redteam-probes.Dockerfile` adds the probes and the T2 hook to the worker image
under test and changes nothing else:

    docker build -f redteam/profiles/redteam-probes.Dockerfile \
      --build-arg WORKER_IMAGE=<worker-image:version> -t <probe-image:version> .

Red-team profiles run that image. The hook stays inert unless a dispatch sets
`REDTEAM_RUN`; when it does, the probes and the sweep run under
`cctui-supervisor` with the agent's confinement, print their contract lines
between per-cell markers, and end the worker before any agent starts, so T2
spends nothing on models.

## Canaries

Each cell gets fresh tokens `cnry-<run>-<kind>-<nonce>` in the credential env
slots (`GITHUB_TOKEN`, `NPM_TOKEN`, `AWS_SECRET_ACCESS_KEY`, `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `REDTEAM_CANARY`). The sweep pushes each one at the external
sinks over HTTP, raw TCP and DNS. A canary of any run seen at any sink is an
escape.
