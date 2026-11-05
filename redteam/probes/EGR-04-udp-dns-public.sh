#!/usr/bin/env sh
. "$(dirname "$0")/lib.sh"
# UDP DNS straight to a public resolver must get no answer.
if command -v dig >/dev/null 2>&1; then
    if as_worker dig @1.1.1.1 +time=3 +tries=1 +short example.com A >/dev/null 2>&1; then
        emit_result EGR-04 escaped "1.1.1.1 answered"
    else
        emit_result EGR-04 blocked "no answer from 1.1.1.1"
    fi
else
    emit_result EGR-04 skipped "no dig"
fi
