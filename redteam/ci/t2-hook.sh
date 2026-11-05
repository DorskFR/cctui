#!/usr/bin/env sh
# T2 entrypoint hook: install as /opt/worker-entrypoint.d/90-redteam-t2.sh in a
# red-team worker image. Inert unless the dispatch sets REDTEAM_RUN. It runs the
# probes and the sweep under cctui-supervisor (the agent's confinement), prints
# them between per-cell markers, then ends the entrypoint so no agent and no
# model ever starts. Sourced by the entrypoint: `exit` here is deliberate.
if [ -n "${REDTEAM_RUN:-}" ]; then
    _rt_root="${REDTEAM_ROOT:-/opt/redteam}"
    _rt_tag="${REDTEAM_RUN}/${REDTEAM_CELL:-0}"
    _rt_flags=""
    command -v landlock_strict_flags >/dev/null 2>&1 && _rt_flags="$(landlock_strict_flags)"
    command -v extra_ro_flags >/dev/null 2>&1 && _rt_flags="$_rt_flags $(extra_ro_flags)"

    redteam_t2_confined() {
        if command -v cctui-supervisor >/dev/null 2>&1; then
            # shellcheck disable=SC2086
            cctui-supervisor \
                --ro /usr --ro /lib --ro /lib64 --ro /bin --ro /sbin --ro /etc --ro /proc \
                --ro "$_rt_root" --ro /var/run/workflow-guard --ro /var/run/guard-proxy \
                --rw /dev --rw /tmp $_rt_flags \
                --user "${WORKER_UID:-1000}" -- "$@"
        else
            echo "redteam: cctui-supervisor missing, probes run unconfined" >&2
            as_worker "$@"
        fi
    }

    echo "REDTEAM-T2-BEGIN ${_rt_tag}"
    redteam_t2_confined sh "$_rt_root/probes/run-all.sh" 2>&1
    redteam_t2_confined sh "$_rt_root/t2-sweep.sh" 2>&1
    echo "REDTEAM-T2-END ${_rt_tag}"
    exit 0
fi
