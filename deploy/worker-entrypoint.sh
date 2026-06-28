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
    if [ -n "${WARM_REPO_DIR:-}" ] && [ -d "$WARM_REPO_DIR" ]; then
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
        _branch_opt=""
        [ -n "${TASK_REPO_REF:-}" ] && _branch_opt="--branch ${TASK_REPO_REF}"
        # shellcheck disable=SC2086  # intentional word-split of the optional flag
        if git clone --depth 1 $_branch_opt "$TASK_REPO_URL" /workspace 2>/dev/null; then
            log "workspace: shallow-cloned TASK_REPO_URL${TASK_REPO_REF:+ @ $TASK_REPO_REF}"
        else
            log "WARNING: TASK_REPO_URL clone failed; /workspace left empty"
        fi
    else
        log "workspace: empty /workspace (no WARM_REPO_DIR / TASK_REPO_URL)"
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
        # Single-token model: one GITHUB_TOKEN in payload.env pulls the pack AND
        # (via the daemon applying payload.env to the session) clones/pushes the
        # work repo. If no dedicated CONTEXT_PACK_TOKEN was given, fall back to it
        # for the pack clone so the tenant ships exactly one credential.
        if [ -z "${CONTEXT_PACK_TOKEN:-}" ]; then
            _gh=$(printf '%s' "$TASK_PAYLOAD_JSON" | jq -r '.env.GITHUB_TOKEN // empty' 2>/dev/null || true)
            [ -n "$_gh" ] && export CONTEXT_PACK_TOKEN="$_gh"
        fi
    fi
    [ -n "${CONTEXT_PACK_URL:-}" ] || { log "context pack: CONTEXT_PACK_URL unset, skipping"; return 0; }
    if [ -z "${CONTEXT_PACK_REF:-}" ]; then
        echo "cctui-worker: CONTEXT_PACK_REF is required when CONTEXT_PACK_URL is set (pin the pack)" >&2
        exit 1
    fi

    _url="$CONTEXT_PACK_URL"
    if [ -n "${CONTEXT_PACK_TOKEN:-}" ]; then
        # Inject an HTTPS basic token (https://<token>@host/...). Never logged.
        _url=$(printf '%s' "$CONTEXT_PACK_URL" | sed "s,^https://,https://${CONTEXT_PACK_TOKEN}@,")
    fi

    _tmp=$(mktemp -d)
    if ! git clone --depth 1 --branch "$CONTEXT_PACK_REF" "$_url" "$_tmp" 2>/dev/null; then
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
    # /opt/context stays root-owned + RO under landlock.
    chown -R 0:0 "$CONTEXT_DIR" 2>/dev/null || true

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
    log "context pack: fetched ref=$CONTEXT_PACK_REF into $CONTEXT_DIR"
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
    mkdir -p "$GUARD_STATE" "$(dirname "$POLICY_FILE")"

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

# ── Run the phases (each individually skippable) ─────────────────────────────
phase_network
phase_workspace
phase_context_pack
phase_credentials
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
#                        Detected per-session (not whole-list emptiness): claude
#                        writes ~/.claude/jobs/<short>/state.json for the job's
#                        lifetime, where short = first 8 chars of SESSION_ID
#                        (control.rs build_dispatch_spec). Gated on "seen alive
#                        once" so a slow cold start is not mistaken for a crash.
#
# Either signal ends the wait; the EXIT trap (phase_callback) then POSTs the
# preserved clean/failed verdict from RESULT_FILE. A non-dispatched (thin)
# worker has no task to finish, so it keeps the original exec-forever behavior.
# GUARD_STATE is already the state FILE path (--state, default
# /var/run/workflow-guard/state) that cctui-guard's engine writes {"step":N} to.
GUARD_STATE_FILE="$GUARD_STATE"
WAIT_POLL_SECS="${WORKER_DONE_POLL_SECS:-2}"

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

# Per-session liveness backstop. claude owns jobs/<short>/state.json; it exists
# while the job lives and is removed when the job ends (the daemon's `claude rm`
# on SessionEnded). We only trust its ABSENCE after we've seen it appear, so a
# cold start that hasn't created it yet doesn't read as a crash.
_JOBS_DIR="${CLAUDE_CONFIG_DIR:-/home/${WORKER_USER}/.claude}/jobs"
_SHORT=$(printf '%s' "${SESSION_ID:-}" | cut -c1-8)
_STATE_JSON="${_JOBS_DIR}/${_SHORT}/state.json"
_SEEN_ALIVE=0
session_dead() {
    if [ -f "$_STATE_JSON" ]; then
        _SEEN_ALIVE=1
        return 1
    fi
    # No state file. A crash AFTER we saw it alive; before that, just not-yet.
    [ "$_SEEN_ALIVE" = 1 ]
}

await_dispatch_done() {
    log "wait: blocking on dual signal (guard=$GUARD_ON, short=${_SHORT:-none})"
    while :; do
        # The daemon process going away is itself terminal — nothing left to
        # finish the task, so stop waiting and let the trap synthesize a verdict.
        if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
            log "wait: cctui-daemon (pid $DAEMON_PID) exited; ending wait"
            return 0
        fi
        if guard_exited; then
            log "wait: guard signalled completion (step=-1)"
            return 0
        fi
        if result_ready; then
            log "wait: result file ready (guard-less done)"
            return 0
        fi
        if session_dead; then
            log "wait: dispatched session ($_SHORT) died without signalling done"
            return 0
        fi
        sleep "$WAIT_POLL_SECS"
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
