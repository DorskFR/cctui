# cctui-supervisor

Privilege-drop exec wrapper for the worker payload. It is the last root step of
the worker entrypoint: it applies Landlock filesystem rules, installs a seccomp
syscall denylist, drops all capabilities and setuids to the worker user, then
`execvp`s the payload command. The seccomp filter and reduced privileges are
inherited by the payload and everything it spawns, and cannot be relaxed.

Rust port of the homelab `landlock-supervisor` Go wrapper.

## Usage

```
cctui-supervisor [OPTIONS] -- <command> [args...]
```

Example:

```
cctui-supervisor \
  --ro /usr --ro /etc \
  --rw /tmp --rw /workspace \
  --user 1000 \
  --report /tmp/hardening.json \
  -- claude --bg
```

## Flags

| Flag            | Description                                                                                  |
| --------------- | -------------------------------------------------------------------------------------------- |
| `--ro <path>`   | Read-only path (repeatable). Overrides the default RO set entirely.                          |
| `--rw <path>`   | Read-write path (repeatable). Overrides the default RW set entirely.                         |
| `--user <uid>`  | Uid (and primary gid) to drop to before exec. Default `1000`.                                |
| `--report <p>`  | Write a JSON hardening report to `p`.                                                         |
| `--strict`      | Treat a missing/unenforced Landlock LSM as fatal instead of warning and continuing.          |
| `--no-seccomp`  | Skip the seccomp filter (debugging only).                                                    |
| `--no-privdrop` | Skip the capability drop / setuid (debugging only).                                          |
| `--dry-run`     | Print the resolved rules and exit without applying anything or exec'ing.                     |

When no `--ro`/`--rw` flags are given, the neutral worker-contract defaults are
used:

- **RO:** `/usr /lib /lib64 /bin /sbin /etc /prompts /opt/context`
- **RW:** `/dev /tmp /workspace /home/worker /var/run/workflow-guard /var/run/guard-proxy`

## Hardening applied

- **Landlock** (best-effort, targets ABI V5 and degrades): restricts the
  filesystem to the RO/RW path sets. RW paths get the V5 write access set, which
  includes `LANDLOCK_ACCESS_FS_IOCTL_DEV` so `claude --bg` can ioctl its PTY.
  Missing/unenforced Landlock warns and continues unless `--strict`.
- **Seccomp denylist** (`EPERM`, default-allow): blocks `ptrace`,
  `process_vm_readv`/`writev`, `setuid`/`setgid`/`setreuid`/`setregid`/
  `setresuid`/`setresgid`, `mount`/`umount2`/`pivot_root`/`chroot`,
  `reboot`/`kexec_load`/`kexec_file_load`/`init_module`/`finit_module`/
  `delete_module`, `unshare`/`setns`. Per-arch tables for `x86_64` and
  `aarch64`. Sets `NO_NEW_PRIVS`.
- **Privilege drop:** clears the inheritable, ambient and bounding capability
  sets, then `setgid`/`setuid` to the target uid (privilege drop happens before
  seccomp, since the filter blocks `setuid`).

## Report shape

```json
{
  "landlock": "V5 (fully-enforced)",
  "seccomp_applied": true,
  "seccomp_blocked": ["ptrace", "..."],
  "caps_dropped": true,
  "uid": 1000,
  "ro_paths": ["/usr", "..."],
  "rw_paths": ["/tmp", "..."],
  "command": ["claude", "--bg"]
}
```

`landlock` is `"unavailable"` when the kernel does not enforce the ruleset.
