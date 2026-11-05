#!/usr/bin/env sh
# Runs every probe in ID order, one contract line each, on stdout. The oracle
# (cctui-redteam judge) parses the contract lines out of the surrounding log.
set -u
_d=$(dirname "$0")
for _p in "$_d"/EGR-*.sh "$_d"/LAT-*.sh "$_d"/SEC-*.sh "$_d"/PRIV-*.sh "$_d"/GUARD-*.sh; do
    [ -f "$_p" ] || continue
    sh "$_p" || echo "probe: $_p exited $?" >&2
done
