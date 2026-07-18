#!/usr/bin/env sh
# Boot wrapper for the guard-proxy sidecar (CCT-721 — remote GPG signing).
#
# The container command is normally just `cctui-guard-proxy <flags>`. This
# wrapper runs FIRST, optionally stands up a gpg-agent that holds the signing
# private key, forwards ONLY the agent's restricted `--extra-socket` into a
# shared emptyDir, then `exec`s cctui-guard-proxy with the SAME flags. It is a
# pure passthrough (just exec the proxy) when GPG_PRIVATE_KEY is absent, so the
# proxy behaves exactly as before when remote signing is off.
#
# Why here and not in the worker: the private key must never enter the worker
# container. The worker only ever sees the restricted extra socket — which can
# USE the key for signing but cannot export it (proven in
# tmp/gpg-forward-test.sh). GPG never touches the network, so the proxy's
# header-injection cannot help; forwarding the agent socket is the mechanism.
#
# The signing key SHOULD be a per-identity signing SUBKEY with a short expiry,
# passphrase-less (a headless sidecar has no pinentry). The mechanism works with
# a subkey: gpg selects the signing-capable subkey automatically.
set -eu

log() { echo "guard-proxy-entrypoint: $*"; }

# Coordinated with the worker entrypoint's wire_gpg_forwarding + the pod's
# `gpg-agent` emptyDir mount. Keep these three in lockstep.
GPG_AGENT_DIR="${CCTUI_GPG_AGENT_DIR:-/var/run/gpg-agent}"
GPG_EXTRA_SOCKET="${CCTUI_GPG_EXTRA_SOCKET:-${GPG_AGENT_DIR}/S.gpg-agent.extra}"

# GUARD_PROXY_GPG_SECRET is a secret REF resolved via the proxy's own engines,
# so no armored key ever sits in the pod spec.
fetch_gpg_key() {
    [ -z "${GPG_PRIVATE_KEY:-}" ] || return 0
    [ -n "${GUARD_PROXY_GPG_SECRET:-}" ] || return 0
    if GPG_PRIVATE_KEY=$(cctui-guard-proxy fetch-secret "$GUARD_PROXY_GPG_SECRET") \
        && [ -n "$GPG_PRIVATE_KEY" ]; then
        export GPG_PRIVATE_KEY
        log "fetched GPG signing key from $GUARD_PROXY_GPG_SECRET"
    else
        unset GPG_PRIVATE_KEY
        log "WARNING: fetch-secret $GUARD_PROXY_GPG_SECRET failed — remote signing unavailable"
    fi
}

start_signing_agent() {
    [ -n "${GPG_PRIVATE_KEY:-}" ] || { log "GPG_PRIVATE_KEY unset — no signing agent (proxy passthrough)"; return 0; }
    command -v gpg >/dev/null 2>&1 || { log "gpg missing — cannot start signing agent"; return 0; }
    command -v gpg-agent >/dev/null 2>&1 || { log "gpg-agent missing — cannot start signing agent"; return 0; }

    # The worker home is not writable by the sidecar uid (1337); keep the private
    # keyring container-local under /tmp.
    _gnupg="${GNUPGHOME:-/tmp/guard-proxy-gnupg}"
    mkdir -p "$_gnupg" && chmod 700 "$_gnupg"
    export GNUPGHOME="$_gnupg"
    mkdir -p "$GPG_AGENT_DIR"

    # The extra-socket path must be set in config BEFORE any agent starts: the
    # import below auto-spawns the agent, which binds the socket from this file.
    printf 'extra-socket %s\n' "$GPG_EXTRA_SOCKET" > "$_gnupg/gpg-agent.conf"
    gpgconf --kill gpg-agent >/dev/null 2>&1 || true

    if ! printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import >/dev/null 2>&1; then
        log "WARNING: failed to import GPG_PRIVATE_KEY — remote signing unavailable"
        unset GPG_PRIVATE_KEY
        return 0
    fi
    # Scrub the armored key from the environment the proxy inherits.
    unset GPG_PRIVATE_KEY

    gpgconf --launch gpg-agent >/dev/null 2>&1 || true
    _i=0
    while [ "$_i" -lt 40 ]; do
        [ -S "$GPG_EXTRA_SOCKET" ] && break
        _i=$((_i + 1)); sleep 0.25
    done
    if [ ! -S "$GPG_EXTRA_SOCKET" ]; then
        log "WARNING: gpg-agent extra socket never appeared at $GPG_EXTRA_SOCKET — remote signing unavailable"
        return 0
    fi
    # fsGroup makes the emptyDir group 1000; open the socket to the group so the
    # worker (uid 1000) can connect. The extra socket still cannot export secrets.
    chmod g+rw "$GPG_EXTRA_SOCKET" 2>/dev/null || true

    # Publish the PUBLIC key + the signing key id for the worker to consume.
    _fpr=$(gpg --list-secret-keys --with-colons 2>/dev/null | awk -F: '$1=="fpr"{print $10; exit}')
    if gpg --export --armor > "${GPG_AGENT_DIR}/pubkey.asc" 2>/dev/null; then
        chmod g+r "${GPG_AGENT_DIR}/pubkey.asc" 2>/dev/null || true
    fi
    if [ -n "$_fpr" ]; then
        printf '%s\n' "$_fpr" > "${GPG_AGENT_DIR}/signingkey"
        chmod g+r "${GPG_AGENT_DIR}/signingkey" 2>/dev/null || true
    fi
    log "gpg-agent up; extra socket forwarded at $GPG_EXTRA_SOCKET (signingkey=${_fpr:-unknown})"
}

fetch_gpg_key
start_signing_agent

exec cctui-guard-proxy "$@"
