#!/usr/bin/env bash
# Exercises the pre-flight isolation check from deploy/worker-entrypoint.sh:
# the runner's verdict per mode, the deny targets, and how a probe reads curl.
set -u

ENTRY="${1:-$(cd "$(dirname "$0")/.." && pwd)/worker-entrypoint.sh}"
fail=0
ok()   { printf '  ok   %s\n' "$1"; }
bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2] got [$3]"; fi; }
has()  { case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: [$2] not in [$3]" ;; esac; }
lacks(){ case "$3" in *"$2"*) bad "$1: [$2] unexpectedly in [$3]" ;; *) ok "$1" ;; esac; }

for f in as_agent cidr_sample preflight_targets preflight_connect preflight_dns \
         preflight_probe preflight_control phase_preflight fail_dispatch; do
  eval "$(awk "/^$f\\(\\)/,/^}/" "$ENTRY")"
  type "$f" >/dev/null 2>&1 || { bad "$f not found in $ENTRY"; exit 1; }
done
eval "$(awk '/^preflight_probe\(\)/,/^}/' "$ENTRY" | sed 's/^preflight_probe()/real_preflight_probe()/')"
eval "$(awk '/^preflight_control\(\)/,/^}/' "$ENTRY" | sed 's/^preflight_control()/real_preflight_control()/')"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
LOG="$TMP/log"
log() { echo "cctui-worker: $*" >> "$LOG"; }

WORKER_UID=1000
WORKER_USER=worker
PREFLIGHT_TIMEOUT=0.3
PREFLIGHT_CONTROL_TIMEOUT=2
CCTUI_BASE_URL=https://cctui.example.dev
TASK_ID=task-1
RESULT_FILE="$TMP/result.json"
TERMINATION_LOG="$TMP/termination-log"
unset KUBERNETES_SERVICE_HOST KUBERNETES_SERVICE_PORT CCTUI_PREFLIGHT_DENY CCTUI_PREFLIGHT_MODE
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY GOOGLE_APPLICATION_CREDENTIALS
unset AWS_WEB_IDENTITY_TOKEN_FILE AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE AZURE_FEDERATED_TOKEN_FILE AWS_CONTAINER_CREDENTIALS_FULL_URI

REACHABLE=""
CONTROL_OK=1
PROBED="$TMP/probed"
preflight_probe() {
  echo "$1 $2" >> "$PROBED"
  [ "$2" = "$REACHABLE" ]
}
preflight_control() { [ "$CONTROL_OK" = 1 ]; }

reset() {
  : > "$LOG"; : > "$PROBED"; rm -f "$RESULT_FILE" "$TERMINATION_LOG"
  REACHABLE=""; CONTROL_OK=1; PREFLIGHT_RESULT=off
  NET_MODE=transparent-external; unset CCTUI_PREFLIGHT_MODE CCTUI_PREFLIGHT_DENY
}

# phase_preflight exits on a fail-closed verdict, so run it in a subshell.
run() { ( phase_preflight; echo "PREFLIGHT_RESULT=$PREFLIGHT_RESULT" >> "$LOG" ); echo $?; }

echo "== 1. every probe denied + control ok -> pass =="
reset
check "rc" 0 "$(run)"
has "logged pass" "preflight: passed" "$(cat "$LOG")"
has "result recorded" "PREFLIGHT_RESULT=passed" "$(cat "$LOG")"
check "no result file" "" "$(cat "$RESULT_FILE" 2>/dev/null)"
has "k8s API probed" "https kubernetes.default.svc:443" "$(cat "$PROBED")"
has "public DNS probed" "dns 1.1.1.1" "$(cat "$PROBED")"
has "IMDS probed" "http 169.254.169.254:80" "$(cat "$PROBED")"
has "IPv6 IMDS probed" "http [fd00:ec2::254]:80" "$(cat "$PROBED")"
has "Pod Identity probed" "http 169.254.170.23:80" "$(cat "$PROBED")"
has "ECS credentials endpoint probed" "http 169.254.170.2:80" "$(cat "$PROBED")"
has "private range probed" "http 10.0.0.1:80" "$(cat "$PROBED")"
has "direct egress probed" "http 1.1.1.1:80" "$(cat "$PROBED")"

echo "== 2. one reachable probe -> fail closed, naming it =="
reset
REACHABLE=kubernetes.default.svc:443
check "rc" 1 "$(run)"
check "result status" failed "$(jq -r .status "$RESULT_FILE" 2>/dev/null)"
check "result task id" task-1 "$(jq -r .task_id "$RESULT_FILE" 2>/dev/null)"
check "reason names the probe" "preflight: k8s API reachable (kubernetes.default.svc:443)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"
check "termination message" "preflight: k8s API reachable (kubernetes.default.svc:443)" \
  "$(cat "$TERMINATION_LOG" 2>/dev/null)"
has "failing probe logged" "preflight: FAIL k8s API reachable" "$(cat "$LOG")"
lacks "agent never reached" "PREFLIGHT_RESULT=" "$(cat "$LOG")"

echo "== 3. warn mode -> continue + logged =="
reset
CCTUI_PREFLIGHT_MODE=warn
REACHABLE=169.254.169.254:80
check "rc" 0 "$(run)"
has "warning logged" "WARNING: preflight: cloud metadata reachable (169.254.169.254:80) (warn mode, continuing)" "$(cat "$LOG")"
has "result recorded" "PREFLIGHT_RESULT=warned" "$(cat "$LOG")"
check "no result file" "" "$(cat "$RESULT_FILE" 2>/dev/null)"

echo "== 4. off -> skipped, nothing probed =="
reset
CCTUI_PREFLIGHT_MODE=off
REACHABLE=kubernetes.default.svc:443
check "rc" 0 "$(run)"
check "no probe ran" "" "$(cat "$PROBED")"
has "result recorded" "PREFLIGHT_RESULT=off" "$(cat "$LOG")"

echo "== 5. proxy path down -> fail closed =="
reset
CONTROL_OK=0
check "rc" 1 "$(run)"
check "reason names the control" \
  "preflight: proxy path to the cctui server failed (https://cctui.example.dev/health)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"

echo "== 6. every failure is named =="
reset
REACHABLE=1.1.1.1:80
CONTROL_OK=0
run >/dev/null
check "both named" \
  "preflight: direct public egress (1.1.1.1:80); proxy path to the cctui server failed (https://cctui.example.dev/health)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"

echo "== 7. unset mode enforces only in transparent-external =="
reset
NET_MODE=forward
REACHABLE=1.1.1.1:80
check "forward warns" 0 "$(run)"
has "warned" "warn mode" "$(cat "$LOG")"
reset
NET_MODE=transparent
REACHABLE=1.1.1.1:80
check "transparent warns" 0 "$(run)"
reset
CCTUI_PREFLIGHT_MODE=enforce
NET_MODE=forward
REACHABLE=1.1.1.1:80
check "explicit enforce wins" 1 "$(run)"
reset
CCTUI_PREFLIGHT_MODE=bogus
REACHABLE=1.1.1.1:80
check "unknown mode enforces" 1 "$(run)"

echo "== 8. profile deny targets =="
reset
CCTUI_PREFLIGHT_DENY=" postgres.db.svc:5432 , 10.42.0.0/16,fd12:3456::/48 "
REACHABLE=10.42.0.1:80
check "rc" 1 "$(run)"
has "host:port probed" "http postgres.db.svc:5432" "$(cat "$PROBED")"
has "IPv6 range probed" "http [fd12:3456::1]:80" "$(cat "$PROBED")"
check "range named" "preflight: profile deny range 10.42.0.0/16 reachable (10.42.0.1:80)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"

echo "== 9. KUBERNETES_SERVICE_HOST is probed =="
reset
KUBERNETES_SERVICE_HOST=10.43.0.1 KUBERNETES_SERVICE_PORT=443 preflight_targets > "$TMP/t"
has "service host" "https 10.43.0.1:443 k8s API reachable" "$(cat "$TMP/t")"
KUBERNETES_SERVICE_HOST=fd00:10::1 KUBERNETES_SERVICE_PORT=6443 preflight_targets > "$TMP/t"
has "IPv6 service host bracketed" "https [fd00:10::1]:6443" "$(cat "$TMP/t")"

echo "== 10. cidr_sample =="
check "/16" 10.42.0.1 "$(cidr_sample 10.42.0.0/16)"
check "host bits cleared" 172.16.0.1 "$(cidr_sample 172.16.9.9/12)"
check "/32 is itself" 192.168.1.7 "$(cidr_sample 192.168.1.7/32)"
check "/0" 0.0.0.1 "$(cidr_sample 0.0.0.0/0)"
check "IPv6" "[fd00::1]" "$(cidr_sample fd00::/8)"

echo "== 11. curl verdicts =="
CURL_RC=0; CURL_TC=0.000000
curl() { printf '%s' "$CURL_TC"; return "$CURL_RC"; }
verdict() { CURL_RC=$1; CURL_TC=$2; NET_MODE=$3; if real_preflight_probe http 10.0.0.1:80; then echo reach; else echo deny; fi; }
check "an answer is reachable"             reach "$(verdict 0 0.001 transparent-external)"
check "a weird reply is reachable"         reach "$(verdict 8 0.001 transparent-external)"
check "proxy close is denied"              deny  "$(verdict 52 0.001 transparent-external)"
check "proxy reset is denied"              deny  "$(verdict 56 0.001 transparent)"
check "TLS refused by proxy is denied"     deny  "$(verdict 35 0.001 transparent-external)"
check "refused is denied"                  deny  "$(verdict 7 0.000000 forward)"
check "timeout before connect is denied"   deny  "$(verdict 28 0.000000 forward)"
check "forward: a connect is reachable"    reach "$(verdict 52 0.004 forward)"
check "forward: slow peer is reachable"    reach "$(verdict 28 0.004 forward)"
unset -f curl

echo "== 12. file probes =="
touch "$TMP/token"; chmod 600 "$TMP/token"
if real_preflight_probe file "$TMP/token"; then ok "readable file reachable"; else bad "readable file"; fi
if real_preflight_probe file "$TMP/missing"; then bad "missing file"; else ok "missing file denied"; fi

echo "== 12b. operator credentials are not ambient identity =="
reset
mkdir -p "$TMP/home/.aws"; : > "$TMP/home/.aws/credentials"
AWS_ACCESS_KEY_ID=AKIA AWS_SECRET_ACCESS_KEY=secret AWS_SESSION_TOKEN=tok \
  GOOGLE_APPLICATION_CREDENTIALS="$TMP/home/adc.json" AZURE_CLIENT_SECRET=s \
  AWS_CONTAINER_CREDENTIALS_RELATIVE_URI=/v2/creds preflight_targets > "$TMP/t"
lacks "no env identity probe" "AWS_ACCESS_KEY_ID" "$(cat "$TMP/t")"
lacks "no credentials file probe" ".aws/credentials" "$(cat "$TMP/t")"
lacks "no ADC probe" "adc.json" "$(cat "$TMP/t")"
export AWS_ACCESS_KEY_ID=AKIA AWS_SECRET_ACCESS_KEY=secret
check "explicit AWS env creds pass" 0 "$(run)"
has "passed" "preflight: passed" "$(cat "$LOG")"
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY

echo "== 12c. pod-provided identity fails closed =="
reset
export AWS_WEB_IDENTITY_TOKEN_FILE="$TMP/token"
REACHABLE="$TMP/token"
check "readable web-identity token fails" 1 "$(run)"
check "reason names it" "preflight: ambient cloud identity via AWS_WEB_IDENTITY_TOKEN_FILE ($TMP/token)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"
unset AWS_WEB_IDENTITY_TOKEN_FILE
reset
REACHABLE=169.254.170.2:80
check "ECS credentials endpoint fails" 1 "$(run)"
AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE=/var/run/pi/token AZURE_FEDERATED_TOKEN_FILE=/var/run/az/token \
  preflight_targets > "$TMP/t"
has "Pod Identity token probed" "file /var/run/pi/token ambient cloud identity via AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE" "$(cat "$TMP/t")"
has "Azure federated token probed" "file /var/run/az/token ambient cloud identity via AZURE_FEDERATED_TOKEN_FILE" "$(cat "$TMP/t")"
full() { AWS_CONTAINER_CREDENTIALS_FULL_URI=$1 preflight_targets | grep FULL_URI | cut -d' ' -f1,2; }
check "full URI with port" "http 169.254.170.23:8080" "$(full http://169.254.170.23:8080/v1/credentials)"
check "full URI default port" "http 169.254.170.23:80" "$(full http://169.254.170.23/v1/credentials)"
check "https default port" "https creds.example:443" "$(full https://creds.example/x)"
check "IPv6 default port" "http [fd00:ec2::23]:80" "$(full 'http://[fd00:ec2::23]/v1/credentials')"
check "IPv6 with port" "http [fd00:ec2::23]:81" "$(full 'http://[fd00:ec2::23]:81/v1')"
REACHABLE=169.254.170.23:8080
export AWS_CONTAINER_CREDENTIALS_FULL_URI=http://169.254.170.23:8080/v1/credentials
check "reachable full URI fails" 1 "$(run)"
has "reason names it" "ambient cloud identity via AWS_CONTAINER_CREDENTIALS_FULL_URI (169.254.170.23:8080)" \
  "$(jq -r .error "$RESULT_FILE" 2>/dev/null)"
unset AWS_CONTAINER_CREDENTIALS_FULL_URI

echo "== 13. control =="
curl() { printf '%s' "$CURL_CODE"; return "$CURL_RC"; }
CURL_CODE=200; CURL_RC=0
if real_preflight_control; then ok "200 passes"; else bad "200"; fi
CURL_CODE=404; CURL_RC=0
if real_preflight_control; then ok "any HTTP answer passes"; else bad "404"; fi
CURL_CODE=000; CURL_RC=7
if real_preflight_control; then bad "no answer"; else ok "no answer fails"; fi
unset -f curl

echo "== 14. it runs after the callback trap and before the daemon =="
_cb=$(grep -n '^phase_callback$' "$ENTRY" | head -1 | cut -d: -f1)
_pf=$(grep -n '^phase_preflight$' "$ENTRY" | head -1 | cut -d: -f1)
_run=$(grep -n '^    run_supervised_daemon "\$@" &$' "$ENTRY" | head -1 | cut -d: -f1)
if [ -n "$_cb" ] && [ -n "$_pf" ] && [ -n "$_run" ] && [ "$_cb" -lt "$_pf" ] && [ "$_pf" -lt "$_run" ]; then
  ok "callback $_cb < preflight $_pf < daemon $_run"
else
  bad "order: callback=$_cb preflight=$_pf daemon=$_run"
fi

exit $fail
