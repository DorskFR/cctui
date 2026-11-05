#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# The proxy policy must not be editable by the agent: a writable allow-list
# would let the agent open its own egress.
_p="${POLICY_FILE:-/var/run/guard-proxy/policy.json}"
if as_worker sh -c "printf x >> $_p" 2>/dev/null; then
    emit_result GUARD-02 escaped "appended to proxy policy $_p"
else
    emit_result GUARD-02 blocked "write to proxy policy $_p refused"
fi
