#!/usr/bin/env bash
# Runs deploy/worker-entrypoint.sh under dash up to the supervisor exec, as the
# real non-root uid and with `id -u` faked to 0, re-rooted into a temp dir with
# the root-only tools shimmed to log their calls. Also covers as_worker.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }
lacks(){ case "$3" in *"$2"*) bad "$1: [$2] unexpectedly in [$3]" ;; *) ok "$1" ;; esac; }

DASH=$(command -v dash) || { echo "dash not installed"; exit 1; }
[ "$(id -u)" != 0 ] || { echo "run as a non-root user"; exit 1; }

TMP=$(mktemp -d)
trap 'chmod -R u+w "$TMP" 2>/dev/null; rm -rf "$TMP"' EXIT

PACK="$TMP/pack"
mkdir -p "$PACK/style"
printf 'pack agents\n' > "$PACK/AGENTS.md"
printf 'style\n' > "$PACK/style/s.md"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git -C "$PACK" init -q
git -C "$PACK" add -A
git -C "$PACK" -c user.name=t -c user.email=t@t commit -qm pack || { bad "pack fixture commit"; exit 1; }

shims() {
  _b="$1/bin"
  mkdir -p "$_b"
  cat > "$_b/id" <<'EOF'
#!/bin/sh
if [ "${1:-}" = -u ] && [ -n "${FAKE_UID:-}" ]; then echo "$FAKE_UID"; exit 0; fi
exec /usr/bin/id "$@"
EOF
  cat > "$_b/setpriv" <<'EOF'
#!/bin/sh
echo "setpriv $*" >> "$CALLS"
while [ $# -gt 0 ]; do case "$1" in --*) shift ;; *) break ;; esac; done
FAKE_UID= exec "$@"
EOF
  cat > "$_b/runuser" <<'EOF'
#!/bin/sh
echo "runuser $*" >> "$CALLS"
while [ $# -gt 0 ]; do [ "$1" = -- ] && { shift; break; }; shift; done
FAKE_UID= exec "$@"
EOF
  for _t in chown update-ca-certificates; do
    printf '#!/bin/sh\necho "%s $*" >> "$CALLS"\n' "$_t" > "$_b/$_t"
  done
  printf '#!/bin/sh\necho "mount $*" >> "$CALLS"\nexit 32\n' > "$_b/mount"
  printf '#!/bin/sh\ncase "$*" in *15002/ready*) exit 0 ;; esac\nexit 7\n' > "$_b/curl"
  printf '#!/bin/sh\nexit 0\n' > "$_b/gpg"
  printf '#!/bin/sh\nexit 0\n' > "$_b/gpgconf"
  printf '#!/bin/sh\necho "SUPERVISOR $*"\necho "supervisor $*" >> "$CALLS"\n' > "$_b/cctui-supervisor"
  chmod +x "$_b"/*
}

setup_root() {
  R="$1"
  mkdir -p "$R/tmp" "$R/var/run/guard-proxy" "$R/var/run/workflow-guard" "$R/var/run/gpg-agent" \
    "$R/var/run/guard-proxy-ca" "$R/workspace" "$R/home/worker" "$R/opt/context" \
    "$R/opt/worker-entrypoint.d" "$R/warm/repo" "$R/overlay"
  printf 'warm\n' > "$R/warm/repo/README"
  printf 'stale\n' > "$R/opt/context/stale"
  printf 'GUARD CA\n' > "$R/var/run/guard-proxy-ca/ca.pem"
  python3 -c 'import socket,sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$R/var/run/gpg-agent/S.gpg-agent.extra"
  cat > "$R/opt/worker-entrypoint.d/25-checkout.sh" <<'EOF'
echo "hook ran as $(as_worker id -u)"
EOF
  shims "$R"
}

run_entry() {
  R="$1"
  sed -e 's#/tmp/#@R@/tmp/#g' -e 's#/var/run/#@R@/var/run/#g' -e 's#/workspace#@R@/workspace#g' \
      -e 's#/opt/#@R@/opt/#g' -e 's#/home/#@R@/home/#g' -e 's#/overlay#@R@/overlay#g' \
      -e 's#/usr/local/share/ca-certificates#@R@/usr/local/share/ca-certificates#g' \
      -e "s#@R@#$R#g" "$ENTRY" > "$R/entry.sh"
  : > "$R/calls"
  env -i PATH="$R/bin:/usr/bin:/bin" HOME="$R/home/worker" CALLS="$R/calls" FAKE_UID="$2" \
    CCTUI_MACHINE_KEY=k CCTUI_URL=http://cctui.test:8700 SESSION_ID=s-1 \
    WORKER_NET_MODE=transparent-external CCTUI_PREFLIGHT_MODE=off \
    TERMINATION_LOG="$R/termination-log" \
    TASK_REPO=repo WARM_REPO_DIR="$R/warm" CONTEXT_PACK_URL="file://$PACK" \
    dash "$R/entry.sh" > "$R/out" 2>&1
  echo "rc=$?" >> "$R/out"
}

echo "== 1. non-root start skips the root-only steps and reaches the supervisor =="
N="$TMP/nonroot"
setup_root "$N"
chmod 555 "$N/opt"
run_entry "$N" ""
out=$(cat "$N/out"); calls=$(cat "$N/calls")
has "exit 0" "rc=0" "$out"
has "announces non-root" "running as uid $(id -u) (non-root)" "$out"
has "supervisor reached as uid 1000" "SUPERVISOR --ro /usr" "$out"
has "supervisor --user 1000" "--user 1000" "$out"
has "CA via bundle, store skipped" "system trust store skipped" "$out"
has "rsync instead of overlay" "workspace: non-root, no overlayfs" "$out"
check "warm cache copied" "warm" "$(cat "$N/workspace/repo/README" 2>/dev/null)"
has "pack fetched" "context pack: fetched" "$out"
check "pack in place under a read-only /opt" "pack agents" "$(cat "$N/opt/context/AGENTS.md" 2>/dev/null)"
check "stale pack file emptied" "no" "$([ -e "$N/opt/context/stale" ] && echo yes || echo no)"
has "bind mounts skipped" "non-root, per-pod home bind mounts skipped" "$out"
has "context stays worker-owned" "stays uid $(id -u)-owned" "$out"
has "hook as_worker runs directly" "hook ran as $(id -u)" "$out"
lacks "no failed hook" "WARNING: extension" "$out"
lacks "no mount" "mount " "$calls"
lacks "no update-ca-certificates" "update-ca-certificates" "$calls"
lacks "no setpriv" "setpriv" "$calls"
lacks "no runuser" "runuser" "$calls"
lacks "no chown to root" "chown -R 0:0" "$calls"

echo "== 2. root start keeps the root steps =="
O="$TMP/root"
setup_root "$O"
run_entry "$O" 0
out=$(cat "$O/out"); calls=$(cat "$O/calls")
has "exit 0" "rc=0" "$out"
lacks "no non-root notice" "non-root" "$out"
has "supervisor reached" "SUPERVISOR --ro /usr" "$out"
has "CA into the system store" "guard-proxy CA installed into system trust store" "$out"
has "update-ca-certificates ran" "update-ca-certificates" "$calls"
has "overlay mount tried" "mount -t overlay" "$calls"
has "overlay fallback logged" "overlayfs unavailable, rsync-copying" "$out"
has "pack home bind mount" "mount --bind" "$calls"
has "pack clone dropped via setpriv" "setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=" "$calls"
has "context chowned to root" "chown -R 0:0 $O/opt/context" "$calls"
check "stale pack file removed" "no" "$([ -e "$O/opt/context/stale" ] && echo yes || echo no)"
has "hook as_worker drops" "hook ran as $(id -u)" "$out"
has "hook drop via setpriv" "setpriv --reuid=1000 --regid=1000 --clear-groups id -u" "$calls"

echo "== 3. as_worker =="
A="$TMP/asw"
mkdir -p "$A"
shims "$A"
awk '/^is_root\(\)/,/^}/' "$ENTRY" > "$A/fns.sh"
awk '/^as_worker\(\)/,/^}/' "$ENTRY" >> "$A/fns.sh"
grep -q '^as_worker()' "$A/fns.sh" || { bad "as_worker not found in $ENTRY"; exit 1; }
asw() {
  env -i PATH="$1" CALLS="$A/calls" FAKE_UID="$2" \
    "$DASH" -c 'set -eu; WORKER_UID=1000; WORKER_USER=worker; . "$1"; as_worker /bin/echo "a b"' run "$A/fns.sh" 2>&1
}
: > "$A/calls"
check "non-root runs directly" "a b" "$(asw "$A/bin:/usr/bin:/bin" "")"
check "non-root: no drop" "" "$(cat "$A/calls")"
: > "$A/calls"
check "root via setpriv" "a b" "$(asw "$A/bin:/usr/bin:/bin" 0)"
check "root: setpriv args" "setpriv --reuid=1000 --regid=1000 --clear-groups /bin/echo a b" "$(cat "$A/calls")"
mkdir -p "$A/nosetpriv"
cp "$A/bin/id" "$A/bin/runuser" "$A/nosetpriv/"
: > "$A/calls"
check "root without setpriv via runuser" "a b" "$(asw "$A/nosetpriv" 0)"
check "root: runuser args" "runuser -u worker -- /bin/echo a b" "$(cat "$A/calls")"

exit $fail
