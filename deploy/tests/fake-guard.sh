# Sourced by the guard-shim tests: start_fake_guard DIR starts fake-guard.py,
# sets GUARD_URL, and stops it on exit. respond STATUS BODY sets the next answer.
start_fake_guard() {
    _fg_dir=$1
    : > "$_fg_dir/requests.log"
    respond 200 '{}'
    python3 -I "$(dirname "${BASH_SOURCE[0]}")/fake-guard.py" \
        "$_fg_dir/response" "$_fg_dir/requests.log" "$_fg_dir/port" &
    FAKE_GUARD_PID=$!
    for _ in $(seq 1 100); do
        [ -s "$_fg_dir/port" ] && break
        sleep 0.05
    done
    [ -s "$_fg_dir/port" ] || { echo "fake guard did not start"; exit 1; }
    GUARD_URL="http://127.0.0.1:$(cat "$_fg_dir/port")"
}

stop_fake_guard() { [ -n "${FAKE_GUARD_PID:-}" ] && kill "$FAKE_GUARD_PID" 2>/dev/null; wait 2>/dev/null; }

respond() { printf '%s\n%s' "$1" "$2" > "$_fg_dir/response"; }

last_request() { tail -n 1 "$_fg_dir/requests.log"; }
