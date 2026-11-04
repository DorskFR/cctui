#!/usr/bin/env bash
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/check-migrations.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

failures=0
check() {
  local name="$1"; shift
  if "$@"; then echo "ok   — $name"; else echo "FAIL — $name"; failures=$((failures + 1)); fi
}
passes() { "$guard" "$dir" >/dev/null 2>&1; }
fails() { ! "$guard" "$dir" >/dev/null 2>&1; }
says() {
  local out
  out="$("$guard" "$dir" 2>&1 || true)"
  grep -qF "$1" <<<"$out"
}

n=0
dir=""
core=""
fresh() {
  n=$((n + 1))
  dir="$tmp/$n"
  core="$dir/crates/cctui-server-core"
  local ws="$dir/crates/cctui-server-workspace"
  mkdir -p "$dir/migrations" "$core/schema" "$core/src" "$ws/schema" "$ws/src" "$dir/crates/cctui-guard/schema"
  touch "$dir/migrations/001_create_sessions.sql" \
    "$dir/migrations/191_file_blobs.up.sql" "$dir/migrations/191_file_blobs.down.sql" \
    "$core/schema/0001_execution.sql" "$core/schema/0002_machine_launches.sql" \
    "$ws/schema/0001_workspace.sql" "$dir/crates/cctui-guard/schema/workflow.v1.json"
  core_entries
  cat >"$ws/src/schema.rs" <<'EOF'
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");
pub async fn provision() {
    install_package(conn, "workspace", &[Migration { version: 1, sql: BOOTSTRAP }], adopted).await
}
EOF
}
core_entries() {
  {
    cat <<'EOF'
pub const BOOTSTRAP: &str = include_str!("../schema/0001_execution.sql");
pub const LEGACY_BASELINE: i64 = 191;
pub struct Migration {
    pub version: i64,
    pub sql: &'static str,
}
pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: BOOTSTRAP },
    Migration {
        version: 2,
        sql: include_str!("../schema/0002_machine_launches.sql"),
    },
EOF
    printf '%s\n' "$@"
    echo '];'
  } >"$core/src/schema.rs"
}
entry() { printf '    Migration { version: %s, sql: include_str!("../schema/%s") },' "$1" "$2"; }

check "the real tree passes" "$guard"

fresh
check "a well-formed tree passes" passes

fresh
touch "$dir/migrations/192_new_table.up.sql" "$dir/migrations/192_new_table.down.sql"
check "a legacy migration past the baseline fails" fails
check "the legacy failure names the file" says "migrations/192_new_table.up.sql"

fresh
touch "$dir/migrations/20261101120000_new_table.up.sql"
check "a timestamped legacy migration fails" fails

fresh
touch "$dir/migrations/notes.txt"
check "a stray file in the legacy dir fails" fails

fresh
touch "$core/schema/0003_execution_stops.sql" "$core/schema/0003_session_viewers.sql"
core_entries "$(entry 3 0003_execution_stops.sql)" "$(entry 3 0003_session_viewers.sql)"
check "two branches adding the same package version fails" fails
check "the duplicate names both files" says "version 3 is used by both 0003_execution_stops.sql and 0003_session_viewers.sql"

fresh
touch "$core/schema/0003_execution_stops.sql" "$core/schema/0003_session_viewers.sql"
core_entries "$(entry 3 0003_execution_stops.sql)" "$(entry 4 0003_session_viewers.sql)"
check "a duplicate file number fails even when registered as distinct versions" fails

fresh
touch "$core/schema/0003_execution_stops.sql"
check "an unregistered package migration fails" fails
check "the unregistered file is named" says "0003_execution_stops.sql: not registered"

fresh
touch "$dir/crates/cctui-server-workspace/schema/0002_tiles.sql"
check "an unregistered workspace migration fails" fails

fresh
touch "$core/schema/0003_execution_stops.sql"
core_entries "$(entry 3 0003_execution_stops.sql)" "$(entry 4 0003_execution_stops.sql)"
check "a migration registered twice fails" fails

fresh
touch "$core/schema/0004_session_name.sql"
core_entries "$(entry 4 0004_session_name.sql)"
check "a gap in a package's versions fails" fails
check "the gap names the missing version" says "version 3 is missing"

fresh
touch "$core/schema/0003_execution_stops.sql"
core_entries "$(entry 4 0003_execution_stops.sql)"
check "a version that does not match its file fails" fails

fresh
touch "$core/schema/0003-execution-stops.sql"
core_entries "$(entry 3 0003-execution-stops.sql)"
check "a malformed package file name fails" fails

fresh
mkdir -p "$dir/crates/cctui-new/schema"
touch "$dir/crates/cctui-new/schema/0001_new.sql"
check "a package stream without src/schema.rs fails" fails

fresh
touch "$core/schema/0003_execution_stops.sql"
core_entries "$(entry 3 0003_execution_stops.sql)"
check "the next package version passes" passes

[ "$failures" -eq 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "all checks passed"
