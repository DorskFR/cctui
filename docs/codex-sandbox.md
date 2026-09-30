# Codex's sandbox on Linux hosts

Codex sandboxes every command it runs under `workspace-write` or `read-only`
with [bubblewrap](https://github.com/containers/bubblewrap), which needs to
create an unprivileged user namespace. Some host policies forbid that, and then
**every** sandboxed command fails.

On Ubuntu 24.04 and later the default
`kernel.apparmor_restrict_unprivileged_userns=1` does exactly this unless an
AppArmor profile grants `bwrap` the capability. The failure looks like:

```
bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted
```

## Why cctui probes for it

Codex does not surface this usefully:

- `codex doctor` reports the sandbox as fine.
- The one signal codex emits is a connection-level `configWarning`, sent when a
  connection initializes. On a shared app-server that happens before any thread
  exists, so it never reaches a session.

A session in `auto` therefore looks healthy and fails every single command. So
the cctui daemon probes the sandbox itself, at codex-adapter start and whenever
a `codex update` moves the binary:

```
codex sandbox -c 'sandbox_mode="workspace-write"' -C <tmpdir> -- /bin/true
```

The verdict rides the daemon heartbeat (`harness_report.codex_sandbox`) and
shows as a badge on the machine in **Settings → harness auto-update**.

## What cctui does with a broken sandbox

| Permission mode | Behaviour |
| --- | --- |
| `auto` | The spawn is **refused** with the fix message. Every command would fail. |
| `ask` (`untrusted`) | Proceeds. Each command is human-approved, and codex reruns a bwrap failure unsandboxed once approved. |
| `yolo` | Unaffected — it asks for `danger-full-access`, so bubblewrap is not used. |

Refusing `auto` is the default because the alternative — silently running with
no sandbox — is a security downgrade. Operators who want it can opt in per
machine in `daemon.toml`:

```toml
[adapters.codex]
codex_sandbox_fallback = "full-access"   # default: "error"
```

Independently of the probe, if a command's output comes back as a `bwrap:`
userns failure, the session gets a one-time notice — that covers a stale probe
and a host policy that changed mid-session.

## Fixing the host

Either install an AppArmor profile for `bwrap` (preferred — it keeps the
restriction on for everything else):

```
# /etc/apparmor.d/bwrap
abi <abi/4.0>,
include <tunables/global>

profile bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,
  include if exists <local/bwrap>
}
```

```sh
sudo apparmor_parser -r /etc/apparmor.d/bwrap
```

Or lift the restriction host-wide, which is blunter:

```sh
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
# persist in /etc/sysctl.d/60-apparmor-namespace.conf
```

Verify with the probe command above: it should exit 0.
