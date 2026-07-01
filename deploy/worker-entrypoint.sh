#!/usr/bin/env sh
# Entrypoint for the cctui worker container (contract v1).
#
# The dispatcher injects the session identity as env (CCTUI_URL,
# CCTUI_MACHINE_KEY, SESSION_ID, TASK_PAYLOAD_JSON, optional REPLY_URL /
# TASK_NAME). The cctui-daemon reads CCTUI_MACHINE_KEY + CCTUI_URL from the
# environment (Config::from_env), so no enroll step and no config file are
# needed here.
#
# The container starts as ROOT to bootstrap the sandbox, then drops to the
# unprivileged `worker` user (uid 1000) via cctui-supervisor before exec'ing the
# daemon. The phases below run root-side; each is INDIVIDUALLY SKIPPABLE on its
# own env, so a worker booted with ONLY CCTUI_URL + CCTUI_MACHINE_KEY +
# SESSION_ID behaves exactly like the thin CCT-245 entrypoint: it falls straight
# through to `exec cctui-supervisor -- cctui-daemon run --no-auto-update`.
#
# No phase may hard-fail when its inputs are absent. The ONE exception is the
# context-pack fetch: if CONTEXT_PACK_URL is set it MUST succeed (the pack
# defines the guard rules — proceeding without it would weaken the sandbox).
#
# Ephemeral worker: --no-auto-update, since the container is short-lived and
# re-fetched on every spawn (k8s/docker workers never self-update — CCT memory).
#
# See docs/worker-contract.md for the full env / mount / capability contract.
set -eu

log() { echo "cctui-worker: $*"; }

# ── Required platform identity ──────────────────────────────────────────────
if [ -z "${CCTUI_MACHINE_KEY:-}" ]; then
    echo "cctui-worker: CCTUI_MACHINE_KEY is required (injected by the dispatcher)" >&2
    exit 1
fi
if [ -z "${CCTUI_URL:-}" ] && [ -z "${CCTUI_SERVER_URL:-}" ]; then
    echo "cctui-worker: CCTUI_URL (or CCTUI_SERVER_URL) is required (injected by the dispatcher)" >&2
    exit 1
fi

# Normalize the server URL we reason about (host extraction, policy seeding).
CCTUI_BASE_URL="${CCTUI_URL:-${CCTUI_SERVER_URL:-}}"

WORKER_UID=1000
WORKER_USER=worker
PROXY_UID=1337
PROXY_PORT=15001
PROXY_HEALTH_PORT=15002
GUARD_PORT=9999
POLICY_FILE=/var/run/guard-proxy/policy.json
GUARD_STATE=/var/run/workflow-guard/state
CONTEXT_DIR=/opt/context

# host:port from a URL (defaulting the port by scheme). Empty on no input.
url_hostport() {
    _u="$1"
    [ -n "$_u" ] || return 0
    _scheme=$(printf '%s' "$_u" | sed -n 's,^\([a-zA-Z][a-zA-Z0-9+.-]*\)://.*,\1,p')
    _hostport=$(printf '%s' "$_u" | sed -e 's,^[a-zA-Z][a-zA-Z0-9+.-]*://,,' -e 's,/.*$,,' -e 's,^[^@]*@,,')
    case "$_hostport" in
        *:*) printf '%s' "$_hostport" ;;
        *)
            case "$_scheme" in
                https) printf '%s:443' "$_hostport" ;;
                http)  printf '%s:80'  "$_hostport" ;;
                *)     printf '%s' "$_hostport" ;;
            esac
            ;;
    esac
}

# bare host (no port) from a URL.
url_host() {
    printf '%s' "$(url_hostport "$1")" | sed 's,:.*$,,'
}

# ── Phase 1: Network mode + guard proxy ─────────────────────────────────────
# transparent (default when CAP_NET_ADMIN): iptables REDIRECT worker egress to
#   the proxy; exempt the proxy uid, root, loopback, the CCTUI_URL host, DNS,
#   and any WORKER_NET_EXEMPT entries; deny IPv6 egress.
# forward (or no NET_ADMIN): no iptables; export HTTP(S)_PROXY for the worker.
# In both modes start cctui-guard-proxy (uid 1337) and seed a deny-default
# policy that always-allows the CCTUI_URL + REPLY_URL hosts.
NET_MODE=""
phase_network() {
    # Decide the mode. Explicit WORKER_NET_MODE wins; else transparent iff we
    # can actually install iptables rules (a proxy for CAP_NET_ADMIN).
    NET_MODE="${WORKER_NET_MODE:-}"
    if [ -z "$NET_MODE" ]; then
        if iptables -t nat -L >/dev/null 2>&1; then
            NET_MODE=transparent
        else
            NET_MODE=forward
        fi
    fi

    mkdir -p "$(dirname "$POLICY_FILE")"

    # Seed a base deny-default policy: allow the structural hosts (the cctui
    # server + the result callback) plus any WORKER_NET_ALLOW hosts. cctui-guard
    # rewrites this per step when a guarded prompt runs (WORKER_NET_ALLOW is
    # re-applied there via --always-allow so it survives every rewrite).
    #
    # WORKER_NET_ALLOW vs WORKER_NET_EXEMPT: ALLOW routes the host THROUGH the
    # proxy and permits it by SNI (IP-independent — the right tool for CDN /
    # multi-IP hosts like a SaaS API). EXEMPT bypasses the proxy via an iptables
    # RETURN on a single boot-resolved IP — only safe for IP-stable hosts.
    _cctui_hp=$(url_hostport "$CCTUI_BASE_URL")
    _reply_hp=$(url_hostport "${REPLY_URL:-}")
    _net_allow=$(printf '%s' "${WORKER_NET_ALLOW:-}" | tr ',' '\n' \
        | sed 's/^[[:space:]]*//;s/[[:space:]]*$//;/^$/d')
    _allowed=$(printf '%s\n%s\n%s\n' "$_cctui_hp" "$_reply_hp" "$_net_allow" | sed '/^$/d' \
        | jq -R . | jq -cs 'unique')
    printf '{"allowed_hosts":%s,"default":"deny"}\n' "$_allowed" > "$POLICY_FILE"
    log "seeded guard-proxy policy (allow: ${_cctui_hp}${_reply_hp:+, $_reply_hp}${WORKER_NET_ALLOW:+, $WORKER_NET_ALLOW}; default deny)"

    if [ "$NET_MODE" = transparent ]; then
        if iptables -t nat -L >/dev/null 2>&1; then
            # Exempt root + proxy uid (avoid a redirect loop), loopback.
            iptables -t nat -A OUTPUT -m owner --uid-owner 0 -j RETURN
            iptables -t nat -A OUTPUT -m owner --uid-owner "$PROXY_UID" -j RETURN
            iptables -t nat -A OUTPUT -d 127.0.0.0/8 -j RETURN

            # DNS (TCP fallback; UDP/53 bypasses the TCP-only REDIRECT already).
            _dns=$(awk '/^nameserver/{print $2; exit}' /etc/resolv.conf 2>/dev/null || true)
            if [ -n "$_dns" ]; then
                iptables -t nat -A OUTPUT -d "$_dns" -j RETURN
                log "iptables: RETURN DNS $_dns"
            fi

            # Operator-plane direct exemptions: comma-separated host:port.
            # These bypass the proxy entirely (resolve to IPs and RETURN).
            if [ -n "${WORKER_NET_EXEMPT:-}" ]; then
                _OLDIFS=$IFS; IFS=,
                for _e in $WORKER_NET_EXEMPT; do
                    IFS=$_OLDIFS
                    _h=$(printf '%s' "$_e" | sed 's,:.*$,,')
                    [ -n "$_h" ] || continue
                    # IPv4 only: this is an IPv4 iptables chain, and getent
                    # hosts would return an AAAA record first for dual-stack
                    # public hosts (e.g. automation.example.internal), which iptables rejects.
                    _ip=$(getent ahostsv4 "$_h" 2>/dev/null | awk '{print $1; exit}' || true)
                    if [ -n "$_ip" ]; then
                        # Don't let one unreachable exempt host (set -e) abort
                        # the whole guard setup — log and carry on.
                        if iptables -t nat -A OUTPUT -d "$_ip" -j RETURN 2>/dev/null; then
                            log "iptables: RETURN exempt $_h ($_ip)"
                        else
                            log "WARNING: iptables exempt rule failed for $_h ($_ip)"
                        fi
                    else
                        log "WARNING: could not resolve WORKER_NET_EXEMPT host $_h"
                    fi
                    IFS=,
                done
                IFS=$_OLDIFS
            fi

            # Everything else from the worker: REDIRECT into the proxy.
            iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner "$WORKER_UID" \
                -j REDIRECT --to-port "$PROXY_PORT"
            log "iptables: worker egress (uid $WORKER_UID) -> :$PROXY_PORT"

            # Deny IPv6 egress: the proxy + REDIRECT are IPv4-only
            # (SO_ORIGINAL_DST), so any IPv6 egress would bypass the filter.
            if ip6tables -L >/dev/null 2>&1; then
                ip6tables -A OUTPUT -o lo -j ACCEPT
                ip6tables -A OUTPUT -j REJECT
                log "ip6tables: IPv6 egress denied (forces IPv4 fallback)"
            else
                log "WARNING: ip6tables unavailable — IPv6 egress is UNFILTERED"
            fi
        else
            log "WARNING: transparent requested but iptables unavailable; falling back to forward"
            NET_MODE=forward
        fi
    fi

    if [ "$NET_MODE" = forward ]; then
        # No iptables: clients must honor the proxy env. Export for the dropped
        # worker tree (the supervisor inherits this env into the daemon).
        export HTTP_PROXY="http://127.0.0.1:${PROXY_PORT}"
        export HTTPS_PROXY="http://127.0.0.1:${PROXY_PORT}"
        export http_proxy="$HTTP_PROXY"
        export https_proxy="$HTTPS_PROXY"
        # Never proxy loopback / the daemon's own control socket.
        export NO_PROXY="127.0.0.1,localhost"
        export no_proxy="$NO_PROXY"
        log "forward mode: HTTP(S)_PROXY -> 127.0.0.1:${PROXY_PORT}"
    fi

    # Start the proxy as uid 1337 (a uid the worker can't write as). setpriv is
    # part of util-linux (present on the base); fall back to running as root if
    # it's missing (still functional, just less isolated).
    if command -v setpriv >/dev/null 2>&1; then
        setpriv --reuid="$PROXY_UID" --regid="$PROXY_UID" --clear-groups \
            cctui-guard-proxy --mode "$NET_MODE" \
                --listen "0.0.0.0:${PROXY_PORT}" \
                --health-listen "0.0.0.0:${PROXY_HEALTH_PORT}" \
                --policy "$POLICY_FILE" &
    else
        cctui-guard-proxy --mode "$NET_MODE" \
            --listen "0.0.0.0:${PROXY_PORT}" \
            --health-listen "0.0.0.0:${PROXY_HEALTH_PORT}" \
            --policy "$POLICY_FILE" &
    fi
    GUARD_PROXY_PID=$!

    # Wait for readiness (policy loaded). Best-effort: don't block boot forever.
    _i=0
    while [ "$_i" -lt 20 ]; do
        if curl -fsS "http://127.0.0.1:${PROXY_HEALTH_PORT}/ready" >/dev/null 2>&1; then
            break
        fi
        _i=$((_i + 1))
        sleep 0.25
    done
    log "guard-proxy started (mode=$NET_MODE, pid $GUARD_PROXY_PID)"
}

# ── Phase 2: Workspace ──────────────────────────────────────────────────────
# WARM_REPO_DIR -> overlayfs (rsync fallback); else TASK_REPO_URL -> shallow
# clone; else an empty /workspace. chown to the worker either way. Skipped (just
# an empty, worker-owned /workspace) when neither var is set.
phase_workspace() {
    mkdir -p /workspace
    _ws_overlay=0
    # Warm cache only when it actually holds this repo (WARM_REPO_DIR/<repo>);
    # an empty/missing cache must fall through to a clone, not overlay nothing.
    if [ -n "${WARM_REPO_DIR:-}" ] && [ -n "${TASK_REPO:-}" ] \
            && [ -d "${WARM_REPO_DIR%/}/${TASK_REPO}" ]; then
        mkdir -p /overlay/upper /overlay/work
        if mount -t overlay overlay \
                -o "lowerdir=${WARM_REPO_DIR},upperdir=/overlay/upper,workdir=/overlay/work" \
                /workspace 2>/dev/null; then
            log "workspace: overlayfs on WARM_REPO_DIR=$WARM_REPO_DIR"
            _ws_overlay=1
        else
            log "workspace: overlayfs unavailable, rsync-copying WARM_REPO_DIR"
            rsync -a "${WARM_REPO_DIR%/}/" /workspace/
        fi
    elif [ -n "${TASK_REPO_URL:-}" ]; then
        # On-demand clone into /workspace/<repo> (matches the warm-overlay layout
        # the prompts expect at /workspace/${TASK_REPO}). Private repos need a
        # token; creds aren't materialized yet at this phase, so resolve the
        # identity token from the pod env and inject it into the clone URL, then
        # scrub it from the persisted remote.
        _dest="/workspace/${TASK_REPO:-repo}"
        mkdir -p "$_dest"
        _wtok="${GITHUB_TOKEN:-}"
        if [ -z "$_wtok" ] && [ -n "${TASK_IDENTITY:-}" ]; then
            _wid=$(printf '%s' "$TASK_IDENTITY" | tr '[:lower:]-' '[:upper:]_')
            eval "_wtok=\${GITHUB_TOKEN_${_wid}:-}"
        fi
        _curl="$TASK_REPO_URL"
        [ -n "$_wtok" ] && _curl=$(printf '%s' "$TASK_REPO_URL" | sed "s,^https://,https://${_wtok}@,")
        if git clone --depth 1 "$_curl" "$_dest" 2>/dev/null; then
            if [ -n "${TASK_REPO_REF:-}" ]; then
                git -C "$_dest" fetch --depth 1 -q origin "$TASK_REPO_REF" 2>/dev/null \
                    && git -C "$_dest" checkout -q FETCH_HEAD 2>/dev/null \
                    || log "WARNING: could not check out TASK_REPO_REF=$TASK_REPO_REF"
            fi
            git -C "$_dest" remote set-url origin "$TASK_REPO_URL" 2>/dev/null || true
            log "workspace: cloned ${TASK_REPO} into $_dest${TASK_REPO_REF:+ @ ${TASK_REPO_REF}}"
        else
            log "WARNING: TASK_REPO_URL clone failed; /workspace left empty"
        fi
    else
        log "workspace: empty /workspace (no warm cache for ${TASK_REPO:-?} / no TASK_REPO_URL)"
    fi
    # Never recursively chown an overlayfs /workspace: it recurses the read-only
    # lowerdir (the whole WARM_REPO_DIR warm cache) and forces a copy-up of every
    # file into the upperdir, wedging boot in NFS I/O over a multi-repo cache
    # (CCT-456). The lowerdir is already worker-readable and writes copy-up into
    # /overlay/upper (chowned below), so the overlay needs no recursive chown —
    # only the root-created fallbacks (rsync/clone/empty) do.
    if [ "$_ws_overlay" = 0 ]; then
        chown -R "${WORKER_UID}:${WORKER_UID}" /workspace 2>/dev/null || true
    fi
    [ -d /overlay/upper ] && chown -R "${WORKER_UID}:${WORKER_UID}" /overlay/upper 2>/dev/null || true
}

# ── Phase 3: Context pack ───────────────────────────────────────────────────
# Operator-plane: CONTEXT_PACK_URL/REF (+ optional TOKEN, SUBDIR). git clone
# --depth 1 the pinned ref into /opt/context (root-side, pre-lockdown), then
# wire its CLAUDE.md / docs / skills / prompts / style / guard-rules.md into the
# locations the agent expects. FAIL-CLOSED: when CONTEXT_PACK_URL is set the
# fetch MUST succeed (the pack defines the guard rules). Skipped entirely when
# CONTEXT_PACK_URL is unset.
#
# Precedence: the pod template (true operator plane) wins. Only when a
# CONTEXT_PACK_* var is NOT already set in the pod env do we fall back to the
# dispatch payload's `env` map (TASK_PAYLOAD_JSON.env, the operator-controlled
# automation dispatcher) — letting a flow select its pack without baking it into the
# template, while a template that pins the pack still overrides the payload.
phase_context_pack() {
    if [ -n "${TASK_PAYLOAD_JSON:-}" ]; then
        for _k in CONTEXT_PACK_URL CONTEXT_PACK_REF CONTEXT_PACK_TOKEN CONTEXT_PACK_SUBDIR; do
            eval "_cur=\${$_k:-}"
            [ -n "$_cur" ] && continue   # pod-template value wins
            _v=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -r --arg k "$_k" '.env[$k] // empty' 2>/dev/null || true)
            [ -n "$_v" ] && export "$_k=$_v"
        done
        # Single-token model: one GITHUB_TOKEN pulls the pack AND (via the daemon
        # applying it to the session) clones/pushes the work repo, so the tenant
        # ships exactly one credential. If no dedicated CONTEXT_PACK_TOKEN was
        # given, resolve a GitHub token for the pack clone, in priority order:
        #   1. payload.env.GITHUB_TOKEN — explicit override (tests / ad-hoc).
        #   2. GITHUB_TOKEN_<IDENTITY> from the pod env — the per-identity secret
        #      Vault injects operator-side (identity-as-root; the job carries only
        #      the `identity` selector, never the secret). Same source the
        #      credentials helper materializes. A future dispatcher-side secret
        #      broker can populate this var without changing the job contract.
        # Operator-named source for the pack-clone token: CONTEXT_PACK_TOKEN_FROM
        # holds the NAME of an env var holding a token that can read the pack repo
        # (e.g. a privileged identity), set in the worker template. The
        # indirection keeps any specific identity name out of this image while
        # letting the operator point the pack clone at a different credential than
        # the task identity (whose token may not have pack-repo access).
        if [ -z "${CONTEXT_PACK_TOKEN:-}" ] && [ -n "${CONTEXT_PACK_TOKEN_FROM:-}" ]; then
            case "$CONTEXT_PACK_TOKEN_FROM" in
                [A-Za-z_][A-Za-z0-9_]*)
                    eval "_v=\${${CONTEXT_PACK_TOKEN_FROM}:-}"
                    [ -n "${_v:-}" ] && export CONTEXT_PACK_TOKEN="$_v" ;;
            esac
        fi
        if [ -z "${CONTEXT_PACK_TOKEN:-}" ]; then
            # Primary: GITHUB_TOKEN already in the pod env — the dispatcher
            # promotes payload.env to pod env and the vault-env webhook resolves
            # any vault: reference before this entrypoint runs, so by here it is a
            # real token. Fallbacks cover non-promoting callers.
            _gh="${GITHUB_TOKEN:-}"
            [ -z "$_gh" ] && _gh=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -r '.env.GITHUB_TOKEN // empty' 2>/dev/null || true)
            if [ -z "$_gh" ]; then
                _id=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -r '.identity // empty' 2>/dev/null || true)
                if [ -n "$_id" ]; then
                    # GITHUB_TOKEN_<ID> with ID upper-cased and non-alnum -> _
                    _idv=$(printf '%s' "$_id" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_')
                    _idv=${_idv%_}
                    eval "_gh=\${GITHUB_TOKEN_${_idv}:-}"
                fi
            fi
            [ -n "$_gh" ] && export CONTEXT_PACK_TOKEN="$_gh"
        fi
    fi
    [ -n "${CONTEXT_PACK_URL:-}" ] || { log "context pack: CONTEXT_PACK_URL unset, skipping"; return 0; }

    # The URL may carry an optional `@<ref>` (in the path) and `#<subdir>`
    # fragment, so a single CONTEXT_PACK_URL can pin ref + subdir; explicit
    # CONTEXT_PACK_REF / CONTEXT_PACK_SUBDIR still take precedence. REF is
    # OPTIONAL — absent ⇒ the remote's default branch (pin it in prod for
    # reproducibility). The `@` is only split out of the path, never the
    # authority, so an embedded `user@host` credential is left intact.
    _raw="$CONTEXT_PACK_URL"
    case "$_raw" in
        *\#*) _frag="${_raw##*#}"; _raw="${_raw%%#*}"
              [ -z "${CONTEXT_PACK_SUBDIR:-}" ] && [ -n "$_frag" ] && CONTEXT_PACK_SUBDIR="$_frag" ;;
    esac
    _scheme=""; _rest="$_raw"
    case "$_raw" in *://*) _scheme="${_raw%%://*}://"; _rest="${_raw#*://}" ;; esac
    case "$_rest" in
        */*) _auth="${_rest%%/*}"; _path="/${_rest#*/}" ;;
        *)   _auth="$_rest"; _path="" ;;
    esac
    case "$_path" in
        *@*) [ -z "${CONTEXT_PACK_REF:-}" ] && CONTEXT_PACK_REF="${_path##*@}"; _path="${_path%@*}" ;;
    esac
    _url="${_scheme}${_auth}${_path}"

    if [ -n "${CONTEXT_PACK_TOKEN:-}" ]; then
        # Inject an HTTPS basic token (https://<token>@host/...). Never logged.
        _url=$(printf '%s' "$_url" | sed "s,^https://,https://${CONTEXT_PACK_TOKEN}@,")
    fi

    _tmp=$(mktemp -d)
    if [ -z "${CONTEXT_PACK_REF:-}" ]; then
        # No ref ⇒ clone the remote's default branch.
        if ! git clone --depth 1 "$_url" "$_tmp" 2>/dev/null; then
            echo "cctui-worker: FATAL context-pack clone failed (default branch)" >&2
            exit 1
        fi
    elif ! git clone --depth 1 --branch "$CONTEXT_PACK_REF" "$_url" "$_tmp" 2>/dev/null; then
        # Some refs are SHAs (not clonable by --branch): fetch the ref directly.
        rm -rf "$_tmp"; _tmp=$(mktemp -d)
        if ! (git -C "$_tmp" init -q \
                && git -C "$_tmp" remote add origin "$_url" \
                && git -C "$_tmp" fetch --depth 1 -q origin "$CONTEXT_PACK_REF" \
                && git -C "$_tmp" checkout -q FETCH_HEAD) 2>/dev/null; then
            echo "cctui-worker: FATAL context-pack fetch failed (CONTEXT_PACK_URL set, ref=$CONTEXT_PACK_REF)" >&2
            exit 1
        fi
    fi

    _src="$_tmp"
    [ -n "${CONTEXT_PACK_SUBDIR:-}" ] && _src="$_tmp/${CONTEXT_PACK_SUBDIR#/}"
    if [ ! -d "$_src" ]; then
        echo "cctui-worker: FATAL context-pack subdir not found: ${CONTEXT_PACK_SUBDIR}" >&2
        exit 1
    fi

    rm -rf "$CONTEXT_DIR"
    mkdir -p "$CONTEXT_DIR"
    # Copy the pack contents (drop .git) into the read-only context dir.
    rm -rf "$_src/.git"
    cp -a "$_src/." "$CONTEXT_DIR/" 2>/dev/null || true
    rm -rf "$_tmp"

    # Wire the pack into the locations the agent expects. Copies (not symlinks)
    # so landlock RO on /opt/context covers them.
    _home="/home/${WORKER_USER}"

    # Per-pod isolation of the home paths the pack overwrites. /home/worker is a
    # ReadWriteMany NFS volume shared across concurrent workers, so writing the
    # pack's CLAUDE.md / projects / style straight onto it would race-corrupt
    # other in-flight dispatches. When a pack is active we bind an empty per-pod
    # dir (under the /overlay emptyDir) over each, so the pack's writes are
    # private to this pod and the shared NFS copy is untouched. (~/.claude is
    # already a per-pod emptyDir, so skills/rules/docs need no isolation.) Best-
    # effort: if /overlay or mount is unavailable we fall through to direct copy.
    if [ -d /overlay ]; then
        _iso="/overlay/pack-home"
        mkdir -p "$_iso"
        for _p in projects style; do
            if [ -d "$CONTEXT_DIR/$_p" ]; then
                mkdir -p "$_iso/$_p" "${_home}/$_p"
                mount --bind "$_iso/$_p" "${_home}/$_p" 2>/dev/null || true
            fi
        done
        if [ -f "$CONTEXT_DIR/CLAUDE.md" ]; then
            : > "$_iso/CLAUDE.md"
            [ -e "${_home}/CLAUDE.md" ] || : > "${_home}/CLAUDE.md"
            mount --bind "$_iso/CLAUDE.md" "${_home}/CLAUDE.md" 2>/dev/null || true
        fi
    fi

    [ -f "$CONTEXT_DIR/CLAUDE.md" ] && cp -f "$CONTEXT_DIR/CLAUDE.md" "${_home}/CLAUDE.md"
    if [ -d "$CONTEXT_DIR/skills" ]; then
        mkdir -p "${_home}/.claude/skills"
        cp -a "$CONTEXT_DIR/skills/." "${_home}/.claude/skills/" 2>/dev/null || true
    fi
    # rules/ = always-on guidance: wire to ~/.claude/rules so Claude Code
    # auto-loads each *.md as instructions on every task (the CCT-490 push seam).
    if [ -d "$CONTEXT_DIR/rules" ]; then
        mkdir -p "${_home}/.claude/rules"
        cp -a "$CONTEXT_DIR/rules/." "${_home}/.claude/rules/" 2>/dev/null || true
    fi
    # docs/ = on-demand reference: wire to ~/.claude/docs so prompts can pull a
    # specific doc by path (@~/.claude/docs/<x>.md). Not auto-loaded.
    if [ -d "$CONTEXT_DIR/docs" ]; then
        mkdir -p "${_home}/.claude/docs"
        cp -a "$CONTEXT_DIR/docs/." "${_home}/.claude/docs/" 2>/dev/null || true
    fi
    if [ -d "$CONTEXT_DIR/style" ]; then
        mkdir -p "${_home}/style"
        cp -a "$CONTEXT_DIR/style/." "${_home}/style/" 2>/dev/null || true
    fi
    if [ -d "$CONTEXT_DIR/projects" ]; then
        mkdir -p "${_home}/projects"
        cp -a "$CONTEXT_DIR/projects/." "${_home}/projects/" 2>/dev/null || true
    fi
    # chown ONLY the paths we just copied in — NOT the whole (NFS-backed) home,
    # which would hang in NFS RPC like the credentials chown (CCT-457).
    for _p in CLAUDE.md .claude/skills .claude/rules .claude/docs style projects; do
        [ -e "${_home}/${_p}" ] \
            && chown -R "${WORKER_UID}:${WORKER_UID}" "${_home}/${_p}" 2>/dev/null || true
    done
    # /opt/context stays root-owned + RO under landlock, but must be world-
    # READABLE/traversable: the dropped-privilege daemon (worker uid) reads the
    # prompt + guard-rules from here. mkdir inherits root's umask (077 -> 700),
    # which blocks the worker from traversing it, so force a+rX.
    chown -R 0:0 "$CONTEXT_DIR" 2>/dev/null || true
    chmod -R a+rX "$CONTEXT_DIR" 2>/dev/null || true

    # Resolve the guard-rules seam. The pack's guard-rules.md is the override/
    # extend layer; any operator-provided GUARD_RULES_FILE becomes the base
    # (GUARD_RULES_BASE), which cctui-guard parses first so the pack can reuse a
    # common set (net-dev, …), extend it (`[name]+:`), or override it (`[name]:`).
    if [ -f "$CONTEXT_DIR/guard-rules.md" ]; then
        if [ -n "${GUARD_RULES_FILE:-}" ] && [ -z "${GUARD_RULES_BASE:-}" ]; then
            export GUARD_RULES_BASE="$GUARD_RULES_FILE"
        fi
        export GUARD_RULES_FILE="$CONTEXT_DIR/guard-rules.md"
    fi
    log "context pack: fetched ref=${CONTEXT_PACK_REF:-<default-branch>} into $CONTEXT_DIR"
}

# Resolve the dispatched prompt file path. TASK_PROMPT_FILE resolves under the
# context pack's prompts/ dir; an absolute path is honored as-is. Echoes the
# resolved path (empty if none).
resolve_prompt_path() {
    [ -n "${TASK_PROMPT_FILE:-}" ] || return 0
    case "$TASK_PROMPT_FILE" in
        /*) printf '%s' "$TASK_PROMPT_FILE" ;;
        *)  printf '%s/prompts/%s' "$CONTEXT_DIR" "$TASK_PROMPT_FILE" ;;
    esac
}

# ── Phase 4: Credentials ────────────────────────────────────────────────────
# Materialize the per-task identity (github/gpg/npm/mcp) into the WORKER home
# via the helper. The helper reads many vars by COMPUTED name (GITHUB_TOKEN_<ID>
# etc.), so we run it inline (env intact) targeting the worker home, then chown
# the products back to the worker — robust across the optional-env matrix and a
# no-op when no credential env is present.
phase_credentials() {
    HOME="/home/${WORKER_USER}" sh /usr/local/bin/cctui-worker-credentials || true
    # chown ONLY the credential products the helper may have written as root —
    # NOT the whole home. The home is NFS-backed and cache-heavy; a recursive
    # chown over it hangs in NFS RPC and the daemon never launches (CCT-457).
    # With no credential env the helper writes nothing, so a full home chown is
    # pure waste; everything else in the home is already worker-owned.
    for _p in .gitconfig .gnupg .npmrc .mcp.json .config/yt; do
        [ -e "/home/${WORKER_USER}/${_p}" ] \
            && chown -R "${WORKER_UID}:${WORKER_UID}" "/home/${WORKER_USER}/${_p}" 2>/dev/null || true
    done
}

# ── Phase 4b: Codex model provider ──────────────────────────────────────────
# The platform injects OPENAI_API_KEY + OPENAI_BASE_URL (the cctui openai
# gateway, CCT-508/514) into the agent env, but the `codex` CLI IGNORES those
# vars — it reads its model provider only from ~/.codex/config.toml. The pod's
# config.toml has just `trust_level = "trusted"` (codex writes that itself), so
# codex's default provider connects straight to api.openai.com with no bearer →
# `401 Unauthorized (Missing bearer)`, and the dual-reviewer review-pr flow
# silently degrades to Claude-only (CCT-517).
#
# When OPENAI_API_KEY is set, MERGE a `[model_providers.cctui]` block + a
# `model_provider = "cctui"` selector into config.toml, pointing codex's
# `responses` wire transport at OPENAI_BASE_URL and reading the bearer from the
# OPENAI_API_KEY env var (codex DOES read env_key from env at request time).
# The block is delimited by BEGIN/END markers and rewritten in place, so this
# is idempotent (safe to re-run) and preserves codex's own keys (trust_level).
# No TOML-aware tool ships in the worker image (jq is JSON-only, no python3), so
# the merge is plain shell: drop any prior cctui-managed region + a stray
# top-level `model_provider`, then append a fresh region. Skipped silently when
# OPENAI_API_KEY is unset.
CODEX_MARKER_BEGIN="# >>> cctui codex model_provider (CCT-517) >>>"
CODEX_MARKER_END="# <<< cctui codex model_provider (CCT-517) <<<"
phase_codex_config() {
    [ -n "${OPENAI_API_KEY:-}" ] || { log "codex: OPENAI_API_KEY unset, skipping model_provider"; return 0; }
    _base="${OPENAI_BASE_URL:-}"
    if [ -z "$_base" ]; then
        log "WARNING: codex: OPENAI_API_KEY set but OPENAI_BASE_URL empty; skipping model_provider"
        return 0
    fi
    _cfgdir="${CODEX_HOME:-/home/${WORKER_USER}/.codex}"
    _cfg="$_cfgdir/config.toml"
    mkdir -p "$_cfgdir"

    # Preserve any existing config MINUS our managed region and a top-level
    # `model_provider` line (we re-set it). awk drops the marker-delimited block;
    # the trailing grep removes a bare `model_provider = …` outside the block.
    _kept=""
    if [ -f "$_cfg" ]; then
        _kept=$(awk -v b="$CODEX_MARKER_BEGIN" -v e="$CODEX_MARKER_END" '
            $0==b {skip=1; next} $0==e {skip=0; next} skip{next} {print}
        ' "$_cfg" | grep -vE '^[[:space:]]*model_provider[[:space:]]*=' || true)
    fi

    {
        [ -n "$_kept" ] && printf '%s\n' "$_kept"
        printf '%s\n' "$CODEX_MARKER_BEGIN"
        printf 'model_provider = "cctui"\n'
        printf '[model_providers.cctui]\n'
        printf 'name = "cctui-gateway"\n'
        printf 'base_url = "%s"\n' "$_base"
        printf 'env_key = "OPENAI_API_KEY"\n'
        printf 'wire_api = "responses"\n'
        printf '%s\n' "$CODEX_MARKER_END"
    } > "$_cfg"
    chown -R "${WORKER_UID}:${WORKER_UID}" "$_cfgdir" 2>/dev/null || true
    log "codex: model_provider 'cctui' wired into $_cfg (base_url from OPENAI_BASE_URL)"
}

# ── Phase 5: Result callback trap ───────────────────────────────────────────
# If REPLY_URL is set, install an EXIT/INT/TERM trap that POSTs the result JSON
# once (RESULT_FILE if the session wrote a valid one, else a synthesized
# failure). Wire shape per files/claude/automation-contract.md — tenant-visible, keep
# identical. Skipped (no trap) when REPLY_URL is unset.
RESULT_FILE="${RESULT_FILE:-/tmp/cctui-result.json}"
export RESULT_FILE
CALLBACK_SENT=0
send_callback() {
    [ "$CALLBACK_SENT" = 1 ] && return 0
    CALLBACK_SENT=1
    [ -z "${REPLY_URL:-}" ] && return 0
    # REPLY_URL is a bearer capability — never log it.
    if curl -4 -sS -X POST "$REPLY_URL" \
        -H 'content-type: application/json' --data "$1" \
        -o /dev/null --connect-timeout 10 --max-time 30 2>/dev/null; then
        log "callback posted"
    else
        log "callback POST failed; caller falls back to its timeout"
    fi
}
worker_on_exit() {
    _code=$?
    if [ -s "$RESULT_FILE" ] && jq -e . "$RESULT_FILE" >/dev/null 2>&1; then
        send_callback "$(cat "$RESULT_FILE")"
    else
        send_callback "$(jq -nc --arg id "${TASK_ID:-}" --arg code "$_code" \
            '{task_id:$id, status:"failed", error:("worker exited (code "+$code+") without a valid result")}')"
    fi
}
phase_callback() {
    [ -n "${REPLY_URL:-}" ] || { log "callback: no REPLY_URL, skipping trap"; return 0; }
    trap worker_on_exit EXIT INT TERM
    log "callback: REPLY_URL trap installed (RESULT_FILE=$RESULT_FILE)"
}

# ── Phase 6: Workflow guard daemon ──────────────────────────────────────────
# If the resolved prompt contains step definitions (`# Step` + `[allowed]`),
# start cctui-guard (root) against the prompt + GUARD_RULES_FILE (default
# /opt/context/guard-rules.md), always-allowing the seeded structural hosts, and
# export the PreToolUse hook env for Claude Code. Skipped when there is no
# prompt or no step markers.
GUARD_ON=off
phase_guard() {
    _prompt=$(resolve_prompt_path)
    [ -n "$_prompt" ] || { log "guard: no prompt, running without guard"; return 0; }
    if [ ! -f "$_prompt" ]; then
        log "guard: prompt $_prompt not found, running without guard"
        return 0
    fi
    # Step markers: a `# Step` heading AND at least one `[allowed]` line.
    if ! grep -qiE '^#+[[:space:]]+step[[:space:]]+[0-9]' "$_prompt" 2>/dev/null \
       || ! grep -qiE '^\[allowed\]' "$_prompt" 2>/dev/null; then
        log "guard: no step markers in prompt, running without guard"
        return 0
    fi

    _rules="${GUARD_RULES_FILE:-$CONTEXT_DIR/guard-rules.md}"
    mkdir -p "$(dirname "$GUARD_STATE")" "$(dirname "$POLICY_FILE")"

    set -- --prompt "$_prompt" \
           --rules "$_rules" \
           --listen "127.0.0.1:${GUARD_PORT}" \
           --state "$GUARD_STATE" \
           --policy-out "$POLICY_FILE"
    # When a pack supplies guard-rules, GUARD_RULES_BASE holds the operator base
    # parsed first; the pack's --rules then overrides/extends it (CCT-490 #6).
    [ -n "${GUARD_RULES_BASE:-}" ] && [ -f "${GUARD_RULES_BASE}" ] \
        && set -- "$@" --rules-base "$GUARD_RULES_BASE"
    # Always-allow the structural hosts so the agent keeps the model gateway and
    # the callback can fire even under a deny-default step policy.
    _cctui_hp=$(url_hostport "$CCTUI_BASE_URL")
    [ -n "$_cctui_hp" ] && set -- "$@" --always-allow "$_cctui_hp"
    _reply_hp=$(url_hostport "${REPLY_URL:-}")
    [ -n "$_reply_hp" ] && set -- "$@" --always-allow "$_reply_hp"
    # Operator-plane SNI allow-list (CDN/multi-IP hosts) must survive every
    # per-step rewrite too, mirroring the seeded policy in phase_network.
    if [ -n "${WORKER_NET_ALLOW:-}" ]; then
        _OLDIFS=$IFS; IFS=,
        for _na in $WORKER_NET_ALLOW; do
            IFS=$_OLDIFS
            _na=$(printf '%s' "$_na" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
            [ -n "$_na" ] && set -- "$@" --always-allow "$_na"
            IFS=,
        done
        IFS=$_OLDIFS
    fi

    cctui-guard "$@" &
    GUARD_PID=$!
    _i=0
    while [ "$_i" -lt 20 ]; do
        curl -fsS "http://127.0.0.1:${GUARD_PORT}/health" >/dev/null 2>&1 && break
        _i=$((_i + 1)); sleep 0.25
    done
    GUARD_ON=on
    # PreToolUse hook env for Claude Code (the agent's settings reference these).
    export CCTUI_GUARD_URL="http://127.0.0.1:${GUARD_PORT}"
    export PROMPT_FILE="$_prompt"
    log "guard: started (prompt=$_prompt, rules=$_rules, pid $GUARD_PID)"
}

# ── Phase 7: Hardening report ───────────────────────────────────────────────
# Collect entrypoint-side state; the supervisor appends landlock/seccomp via
# --report. Exposed as WORKER_HARDENING_JSON (env) + a file the daemon can
# attach as session metadata.
HARDENING_FILE=/tmp/cctui-hardening.json
phase_hardening() {
    WORKER_HARDENING_JSON=$(jq -nc \
        --arg net "$NET_MODE" \
        --arg guard "$GUARD_ON" \
        '{net_mode:$net, guard:$guard, supervisor_report:"/tmp/hardening.json"}')
    export WORKER_HARDENING_JSON
    printf '%s\n' "$WORKER_HARDENING_JSON" > "$HARDENING_FILE"
    export WORKER_HARDENING_FILE="$HARDENING_FILE"
    log "hardening: net_mode=$NET_MODE guard=$GUARD_ON"
}

# ── Phase 7b: Permission bypass seeding ─────────────────────────────────────
# A dispatched headless session runs in claude's bypassPermissions mode, which
# REFUSES to start (prompts the disclaimer) until `bypassPermissionsModeAccepted`
# is recorded in .claude.json, and prompts a trust dialog until
# `projects.<cwd>.hasTrustDialogAccepted` is set. The per-pod .claude is a fresh
# emptyDir every pod, so we seed both. Belt-and-suspenders in settings.json:
# `skipDangerousModePermissionPrompt` (the disclaimer) + `permissions.defaultMode
# = bypassPermissions` (unattended, no prompts). Merged into any existing files
# so the daemon's --settings hooks and managed-settings are untouched. (CCT-475)
phase_permissions() {
    _cfgdir="${CLAUDE_CONFIG_DIR:-/home/${WORKER_USER}/.claude}"
    _cwd="${CCTUI_DISPATCH_WORKDIR:-/workspace}"
    mkdir -p "$_cfgdir"
    _cfg="$_cfgdir/.claude.json"
    if [ -f "$_cfg" ]; then
        _t=$(mktemp) && jq --arg cwd "$_cwd" \
            '. + {bypassPermissionsModeAccepted: true}
             | .projects = ((.projects // {}) | .[$cwd] = ((.[$cwd] // {}) + {hasTrustDialogAccepted: true}))' \
            "$_cfg" > "$_t" && mv "$_t" "$_cfg"
    else
        jq -nc --arg cwd "$_cwd" \
            '{bypassPermissionsModeAccepted: true, projects: {($cwd): {hasTrustDialogAccepted: true}}}' \
            > "$_cfg"
    fi
    _settings="$_cfgdir/settings.json"
    if [ -f "$_settings" ]; then
        _t=$(mktemp) && jq --arg cwd "$_cwd" \
            '. + {skipDangerousModePermissionPrompt: true}
             | .permissions = ((.permissions // {}) + {defaultMode: "bypassPermissions"}
               | .additionalDirectories = (((.additionalDirectories // []) + [$cwd]) | unique))' \
            "$_settings" > "$_t" && mv "$_t" "$_settings"
    else
        jq -nc --arg cwd "$_cwd" \
            '{skipDangerousModePermissionPrompt: true, permissions: {defaultMode: "bypassPermissions", additionalDirectories: [$cwd]}}' \
            > "$_settings"
    fi
    # .claude is a per-pod emptyDir (not the NFS home), so a recursive chown is
    # safe here (unlike the home — CCT-457).
    chown -R "${WORKER_UID}:${WORKER_UID}" "$_cfgdir" 2>/dev/null || true
    log "permissions: seeded bypass + trust gates (cwd=$_cwd)"
}

# ── Identity secret surface (CCT-490, simple model) ─────────────────────────
# The pod env carries every identity's third-party secrets as `VAR_<ID>` (Vault
# env-from-path) plus any unsuffixed defaults. Two steps around the credential
# helper, keyed on the active identity (`<ID>` = TASK_IDENTITY uppercased,
# `-`→`_`):
#   resolve (before the helper): collapse each base to the active identity's
#     "main" value — `VAR = ${VAR_<ID>:-$VAR}` — so the helper and the agent read
#     one canonical var.
#   scrub (after the helper): UNSET every `VAR_<…>` suffixed variant so the agent
#     inherits only the resolved mains, never another identity's secret.
SECRET_BASES="GITHUB_TOKEN GH_TOKEN GITHUB_NAME GITHUB_EMAIL GPG_PRIVATE_KEY YOUTRACK_TOKEN YOUTRACK_API_TOKEN SLACK_TOKEN"
phase_identity_resolve() {
    [ -n "${TASK_IDENTITY:-}" ] || return 0
    _id_up=$(printf '%s' "$TASK_IDENTITY" | tr '[:lower:]-' '[:upper:]_')
    for _base in $SECRET_BASES; do
        eval "_v=\${${_base}_${_id_up}:-}"
        [ -n "${_v:-}" ] && export "${_base}=${_v}"
    done
    log "identity resolve: canonical secrets set for ${TASK_IDENTITY}"
}
phase_identity_scrub() {
    for _base in $SECRET_BASES; do
        for _var in $(env | sed -n "s/^\\(${_base}_[A-Za-z0-9_]*\\)=.*/\\1/p"); do
            unset "$_var" 2>/dev/null || true
        done
    done
    log "identity scrub: per-identity secret variants removed from the agent env"
}

# ── Run the phases (each individually skippable) ─────────────────────────────
# Derive the task-shape env the prompts + workspace expect from the dispatch
# payload. The dispatcher forwards the whole payload as TASK_PAYLOAD_JSON but
# only injects a few magic vars, so map the conventional fields here. Each is
# only set when unset, so an explicit pod-template/dispatcher value still wins.
if [ -n "${TASK_PAYLOAD_JSON:-}" ]; then
    _pj() { printf '%s' "$TASK_PAYLOAD_JSON" | jq -r "$1 // empty" 2>/dev/null || true; }
    [ -z "${TASK_IDENTITY:-}" ] && { _v=$(_pj '.identity'); [ -n "$_v" ] && export TASK_IDENTITY="$_v"; }
    [ -z "${TASK_REPO:-}" ]     && { _v=$(_pj '.repo');     [ -n "$_v" ] && export TASK_REPO="$_v"; }
    [ -z "${TASK_EFFORT:-}" ]   && { _v=$(_pj '.effort');   [ -n "$_v" ] && export TASK_EFFORT="$_v"; }
    [ -z "${TASK_MODEL:-}" ]    && { _v=$(_pj '.model');    [ -n "$_v" ] && export TASK_MODEL="$_v"; }
    if [ -z "${TASK_CONTEXT_JSON:-}" ]; then
        _v=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -c '.context // empty' 2>/dev/null || true)
        [ -n "$_v" ] && export TASK_CONTEXT_JSON="$_v"
    fi
    # Repo acquisition (no warm cache ⇒ clone): build the URL from context.owner +
    # repo, and the ref from context.head_sha, unless explicitly provided.
    if [ -z "${TASK_REPO_URL:-}" ] && [ -n "${TASK_REPO:-}" ]; then
        _owner=$(_pj '.context.owner')
        [ -n "$_owner" ] && export TASK_REPO_URL="https://github.com/${_owner}/${TASK_REPO}"
    fi
    [ -z "${TASK_REPO_REF:-}" ] && { _v=$(_pj '.context.head_sha'); [ -n "$_v" ] && export TASK_REPO_REF="$_v"; }
fi
phase_network
phase_workspace
phase_context_pack
# When a pack is active, drive cctui-guard from the dispatched prompt: derive
# TASK_PROMPT_FILE from payload.prompt_file so resolve_prompt_path finds the
# pack's prompt and the guard enforces its [allowed]/[network] steps. Scoped to
# pack flows (CONTEXT_PACK_URL set) so legacy configMap dispatches keep their
# current — unguarded — behavior until they migrate to a pack.
if [ -z "${TASK_PROMPT_FILE:-}" ] && [ -n "${CONTEXT_PACK_URL:-}" ] && [ -n "${TASK_PAYLOAD_JSON:-}" ]; then
    TASK_PROMPT_FILE=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -r '.prompt_file // empty' 2>/dev/null || true)
    [ -n "${TASK_PROMPT_FILE:-}" ] && export TASK_PROMPT_FILE \
        && log "prompt: TASK_PROMPT_FILE=${TASK_PROMPT_FILE} (from payload; pack active → guard will engage if the prompt has steps)"
fi
phase_identity_resolve
phase_credentials
phase_codex_config
phase_identity_scrub
phase_callback
phase_guard
phase_permissions
phase_hardening

# ── Phase 8: Drop privileges + run ──────────────────────────────────────────
# cctui-supervisor applies landlock + seccomp, drops all caps, setuids to the
# worker, then exec's the daemon. RO: system + context/prompts; RW: workspace,
# home, tmp, the guard/proxy state dirs. The supervisor's own --report captures
# landlock/seccomp/uid for the hardening metadata.
run_supervised_daemon() {
    cctui-supervisor \
        --ro /usr --ro /lib --ro /lib64 --ro /bin --ro /sbin --ro /etc --ro /proc \
        --ro /prompts \
        --ro "$CONTEXT_DIR" \
        --rw /dev --rw /tmp --rw /workspace --rw "/home/${WORKER_USER}" \
        --rw /var/run/workflow-guard --rw /var/run/guard-proxy \
        --user "$WORKER_UID" \
        --report /tmp/hardening.json \
        -- cctui-daemon run --no-auto-update "$@"
}

# ── Phase 9: Dual-signal "work done" wait (CCT-483) ─────────────────────────
# With `claude -p`, "work is done" (semantic) and "process is gone" (liveness)
# were the same event, so `wait $PID` sufficed. `claude daemon` splits them:
# the dispatch op acks instantly and the daemon stays up — there is no blocking
# "wait until done" primitive. So a DISPATCHED worker (SESSION_ID +
# TASK_PAYLOAD_JSON present) runs the daemon in the BACKGROUND and blocks on two
# signals:
#
#   PRIMARY (done)     — the agent declares completion via cctui-guard ->
#                        POST /transition {"step":"exit"}, which flips the guard
#                        state file to STEP_EXITED (-1) AND relaxes the egress
#                        proxy so the result callback can leave. Guard-less
#                        workers (no step markers) fall back to watching the
#                        RESULT_FILE appear with valid JSON.
#   BACKSTOP (crashed) — the dispatched session dies WITHOUT signalling done.
#                        Sourced from the daemon's authoritative registration:
#                        the server row for $SESSION_ID leaves "active" (the
#                        daemon deregistered it on SessionEnded, or its heartbeat
#                        went stale). Gated on "seen registered once" so a slow
#                        cold start is not mistaken for a crash (CCT-521).
#
# Either signal ends the wait; the EXIT trap (phase_callback) then POSTs the
# preserved clean/failed verdict from RESULT_FILE. A non-dispatched (thin)
# worker has no task to finish, so it keeps the original exec-forever behavior.
# GUARD_STATE is already the state FILE path (--state, default
# /var/run/workflow-guard/state) that cctui-guard's engine writes {"step":N} to.
GUARD_STATE_FILE="$GUARD_STATE"
WAIT_POLL_SECS="${WORKER_DONE_POLL_SECS:-2}"
# Fail-closed boot bound (CCT-520). A `claude daemon run` that crash-loops
# `exited code=1` (often behind a network deny) keeps cctui-daemon up but never
# dispatches a session, so it never registers and the backstop (gated on "seen
# registered once") never fires — the wait would block to the 24h
# activeDeadlineSeconds, burning a pod slot for a day with no callback. Bound the
# time-to-first-registration: if the dispatched session never registers within
# this window, write a failed RESULT_FILE and exit non-zero so the EXIT trap
# POSTs the callback promptly.
WORKER_BOOT_DEADLINE_SECS="${WORKER_BOOT_DEADLINE_SECS:-120}"

# Guard signalled completion: state file says {"step":-1}.
guard_exited() {
    [ "$GUARD_ON" = on ] || return 1
    [ -f "$GUARD_STATE_FILE" ] || return 1
    _step=$(jq -r '.step // empty' "$GUARD_STATE_FILE" 2>/dev/null || true)
    [ "$_step" = "-1" ]
}

# Guard-less done: the session wrote a valid result JSON.
result_ready() {
    [ "$GUARD_ON" = on ] && return 1
    [ -s "$RESULT_FILE" ] && jq -e . "$RESULT_FILE" >/dev/null 2>&1
}

# Per-session liveness backstop — sourced from the cctui-daemon's own
# registration, NOT a grep of claude's private jobs dir (CCT-521). The old
# backstop keyed on ~/.claude/jobs/<short>/state.json, i.e. claude's INTERNAL
# job id. Claude rotates that id on resume/clear (CCT-160) and never writes it
# at the guessed path for cold-dispatched worker sessions, so _SEEN_ALIVE stayed
# 0 and the 120s boot bound below guillotined fully-alive reviews mid-work (live
# incident: PR #5679/#5776 — killed at exactly 120s while streaming to the API
# and mid guard Step 2). The daemon is the source of truth: it launches claude
# with `--session-id $SESSION_ID` (control.rs, CCT-446) and registers THAT stable
# id with the server, immune to claude's id rotation. Ask the server for it.
_SESSION_URL="${CCTUI_BASE_URL%/}/api/v1/sessions/${SESSION_ID}"
_PROBE_BODY=/tmp/cctui-liveness-probe.json
WORKER_LIVENESS_POLL_SECS="${WORKER_LIVENESS_POLL_SECS:-10}"
_SEEN_ALIVE=0
# Probe the daemon's server-side registration for OUR session id. Echoes:
#   registered — HTTP 200 and status "active": the daemon holds this session
#                live in its registry (heartbeat fresh within STATUS_WINDOW=5m).
#   ended      — HTTP 200 but status != "active" (daemon deregistered it on
#                SessionEnded -> DB row goes 'inactive', or heartbeat stale >5m),
#                or HTTP 404 (row deleted). Only trusted as death after we have
#                seen it registered at least once.
#   unknown    — transient curl/HTTP error or not-yet-registered; never death.
# `-4` mirrors the callback curl (the worker forces IPv4 egress; CCT-468).
probe_session() {
    _code=$(curl -4 -sS -o "$_PROBE_BODY" -w '%{http_code}' --max-time 5 \
        -H "Authorization: Bearer $CCTUI_MACHINE_KEY" "$_SESSION_URL" 2>/dev/null) \
        || { echo unknown; return; }
    case "$_code" in
        200)
            _st=$(jq -r '.status // empty' "$_PROBE_BODY" 2>/dev/null || true)
            [ "$_st" = active ] && echo registered || echo ended
            ;;
        404) echo ended ;;
        *) echo unknown ;;
    esac
}

await_dispatch_done() {
    log "wait: blocking on dual signal (guard=$GUARD_ON, session=${SESSION_ID:-none})"
    _waited=0
    _next_probe=0
    while :; do
        # The daemon process going away is itself terminal — nothing left to
        # finish the task, so stop waiting and let the trap synthesize a verdict.
        if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
            log "wait: cctui-daemon (pid $DAEMON_PID) exited; ending wait"
            return 0
        fi
        # PRIMARY done-signals — cheap local file reads, every loop.
        if guard_exited; then
            log "wait: guard signalled completion (step=-1)"
            return 0
        fi
        if result_ready; then
            log "wait: result file ready (guard-less done)"
            return 0
        fi
        # Daemon-sourced liveness — throttled server probe (not every 2s).
        if [ "$_waited" -ge "$_next_probe" ]; then
            _next_probe=$((_waited + WORKER_LIVENESS_POLL_SECS))
            case "$(probe_session)" in
                registered)
                    if [ "$_SEEN_ALIVE" = 0 ]; then
                        _SEEN_ALIVE=1
                        log "wait: session ${SESSION_ID%%-*} registered with the daemon (seen alive)"
                    fi
                    ;;
                ended)
                    # Only terminal once we have seen it alive: a pre-registration
                    # 404 is just a slow cold start, not a crash.
                    if [ "$_SEEN_ALIVE" = 1 ]; then
                        log "wait: daemon reports session ${SESSION_ID%%-*} ended without a done-signal"
                        return 0
                    fi
                    ;;
            esac
        fi
        # Fail-closed boot bound (CCT-520/521): the daemon is still up but never
        # registered the dispatched session within the boot window — a wedged
        # `claude daemon run` (crash-loop / network deny). Surface a fast failure
        # instead of blocking to the 24h deadline. Once the session has been seen
        # alive ($_SEEN_ALIVE=1), the bound no longer applies; long legitimate
        # work is governed by activeDeadlineSeconds as before.
        if [ "$_SEEN_ALIVE" = 0 ] && [ "$_waited" -ge "$WORKER_BOOT_DEADLINE_SECS" ]; then
            log "wait: claude daemon failed to boot a session within ${WORKER_BOOT_DEADLINE_SECS}s; failing closed"
            jq -nc --arg id "${TASK_ID:-}" --arg secs "$WORKER_BOOT_DEADLINE_SECS" \
                '{task_id:$id, status:"failed", error:("claude daemon failed to boot a session within "+$secs+"s")}' \
                > "$RESULT_FILE"
            exit 1
        fi
        sleep "$WAIT_POLL_SECS"
        _waited=$((_waited + WAIT_POLL_SECS))
    done
}

if [ -n "${SESSION_ID:-}" ] && [ -n "${TASK_PAYLOAD_JSON:-}" ]; then
    log "dispatched worker -> background cctui-daemon + dual-signal wait (CCT-483)"
    run_supervised_daemon "$@" &
    DAEMON_PID=$!
    await_dispatch_done
    # Best-effort: stop the daemon so the container winds down promptly. The
    # EXIT trap (phase_callback) then POSTs the preserved RESULT_FILE verdict.
    kill "$DAEMON_PID" 2>/dev/null || true
    exit 0
fi

log "thin worker -> exec cctui-supervisor -> cctui-daemon (run forever)"
exec cctui-supervisor \
    --ro /usr --ro /lib --ro /lib64 --ro /bin --ro /sbin --ro /etc --ro /proc \
    --ro /prompts \
    --ro "$CONTEXT_DIR" \
    --rw /dev --rw /tmp --rw /workspace --rw "/home/${WORKER_USER}" \
    --rw /var/run/workflow-guard --rw /var/run/guard-proxy \
    --user "$WORKER_UID" \
    --report /tmp/hardening.json \
    -- cctui-daemon run --no-auto-update "$@"
