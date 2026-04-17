# JSONL Archive — Per-Machine Backup Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Push every `~/.claude/projects/**/*.jsonl` transcript from every machine to a single server-side PVC, laid out byte-identically to the machine's on-disk tree, so any machine can be restored by rsync.

**Architecture:** Channel discovers JSONL files, hashes them, calls `HEAD` to skip unchanged uploads, `PUT` full bytes when changed. Server writes to `<CCTUI_ARCHIVE_PATH>/<machine_uuid>/projects/<project_dir>/<session_id>.jsonl` via `.partial` + atomic rename, indexes row in `archive_index`. Triggers: startup scan (all projects), periodic while session live (15 min default), final flush on SIGTERM/SIGINT. All archive routes require `Machine` role — `machine_id` comes from auth context, never URL.

**Tech Stack:** Rust (axum, sqlx, tokio, sha2, hex) server-side; TypeScript (Bun, node:crypto, node:fs) channel-side; PostgreSQL migration.

**Ticket:** CCT-45. Depends on CCT-36 PR1 (already merged — `machines` table + `TokenRole::Machine` + `AuthContext.machine_id` exist). Channel-side `machine.json` loader lands with CCT-36 PR2 (#7, in review) — this plan keeps channel compat with both old (`CCTUI_AGENT_TOKEN`) and new (`machine.json`) token sources so it merges independently.

---

## File Structure

**New:**
- `migrations/009_archive_index.sql` — `archive_index` table + indexes.
- `crates/cctui-server/src/archive_store.rs` — filesystem layer: stream-write, sha256, atomic rename, path safety.
- `crates/cctui-server/src/routes/archive.rs` — `HEAD`, `PUT`, (optional) `GET` routes.
- `channel/src/archive.ts` — project walker, sha256 hasher, `uploadIfChanged` orchestrator + in-proc hash cache.
- `channel/test/archive.test.ts` — walker + hasher unit tests.

**Modified:**
- `crates/cctui-server/src/config.rs` — add `archive_path: PathBuf` field (env `CCTUI_ARCHIVE_PATH`, default `/archive`).
- `crates/cctui-server/src/state.rs` — carry `ArchiveStore` on `AppState`.
- `crates/cctui-server/src/main.rs` — construct store, register routes on `api_router`, `mkdir_p` on boot.
- `crates/cctui-server/src/routes/mod.rs` — `pub mod archive;`.
- `crates/cctui-server/tests/integration.rs` — add archive flow tests.
- `channel/src/bridge.ts` — `headArchive`, `putArchive` methods.
- `channel/src/index.ts` — wire startup scan, periodic interval, final flush in SIGTERM/SIGINT.
- `STATUS.md` — mark archive "done, file-backed backup".

---

## Task 1: DB migration `009_archive_index.sql`

**Files:**
- Create: `migrations/009_archive_index.sql`

- [ ] **Step 1: Write migration**

```sql
CREATE TABLE archive_index (
    id             BIGSERIAL PRIMARY KEY,
    machine_id     UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    project_dir    TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    sha256         TEXT NOT NULL,
    size_bytes     BIGINT NOT NULL,
    line_count     INTEGER,
    uploaded_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (machine_id, session_id)
);
CREATE INDEX idx_archive_machine_project ON archive_index(machine_id, project_dir);
CREATE INDEX idx_archive_session ON archive_index(session_id);
```

- [ ] **Step 2: Apply to local DB**

Run: `make setup` (or `sqlx migrate run` with `DATABASE_URL` pointing at local Postgres).
Expected: migration 009 listed as applied, no errors.

- [ ] **Step 3: Commit**

```bash
git add migrations/009_archive_index.sql
git commit -m "feat(db): archive_index table for per-machine JSONL backups (CCT-45)"
```

---

## Task 2: `ArchiveStore` — filesystem layer (TDD)

**Files:**
- Create: `crates/cctui-server/src/archive_store.rs`
- Modify: `crates/cctui-server/src/main.rs` (`mod archive_store;` + construct)
- Modify: `crates/cctui-server/src/state.rs` (add `pub archive: Arc<ArchiveStore>` field)

- [ ] **Step 1: Write the failing unit tests**

Append to `crates/cctui-server/src/archive_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use uuid::Uuid;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn write_roundtrip_hashes_and_renames() {
        let dir = tmp();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        let body: &[u8] = b"{\"a\":1}\n{\"b\":2}\n";

        let stats = store
            .write(machine, "-home-user-proj", "abc-123", body)
            .await
            .expect("write ok");

        assert_eq!(stats.size_bytes, body.len() as u64);
        assert_eq!(stats.line_count, 2);
        // sha256 of body
        let expected =
            "18bd4b7c4c7e6f2e0e6...".to_string(); // replace with real hash in step 3
        assert_eq!(stats.sha256.len(), 64);
        let path = store.path_of(machine, "-home-user-proj", "abc-123");
        assert!(path.exists());
        assert!(!path.with_extension("jsonl.partial").exists());
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let dir = tmp();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        let err = store
            .write(machine, "../evil", "abc", b"".as_slice())
            .await
            .unwrap_err();
        assert!(matches!(err, ArchiveError::InvalidName));
    }

    #[tokio::test]
    async fn rejects_slashes_and_null_in_names() {
        let dir = tmp();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        assert!(store.write(machine, "a/b", "abc", b"".as_slice()).await.is_err());
        assert!(store.write(machine, "ok", "a/b", b"".as_slice()).await.is_err());
        assert!(store.write(machine, "ok", "a\0b", b"".as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn partial_cleaned_up_on_body_err() {
        use tokio::io::{AsyncRead, ReadBuf};
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct BadReader;
        impl AsyncRead for BadReader {
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                _: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Err(std::io::Error::other("boom")))
            }
        }

        let dir = tmp();
        let store = ArchiveStore::new(dir.path().to_path_buf());
        let machine = Uuid::new_v4();
        let _ = store
            .write(machine, "-home-user-proj", "abc-123", BadReader)
            .await;
        let base = store.path_of(machine, "-home-user-proj", "abc-123");
        assert!(!base.exists());
        let partial = base.with_extension("jsonl.partial");
        assert!(!partial.exists());
    }
}
```

Add `tempfile = "3"` to `[dev-dependencies]` in `crates/cctui-server/Cargo.toml`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cctui-server archive_store`
Expected: module not found / type missing errors.

- [ ] **Step 3: Implement `ArchiveStore`**

Write `crates/cctui-server/src/archive_store.rs`:

```rust
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("invalid name")]
    InvalidName,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub sha256: String,
    pub size_bytes: u64,
    pub line_count: u32,
}

#[derive(Debug, Clone)]
pub struct ArchiveStore {
    root: PathBuf,
}

impl ArchiveStore {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn ensure_root(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.root).await
    }

    #[must_use]
    pub fn path_of(&self, machine_id: Uuid, project_dir: &str, session_id: &str) -> PathBuf {
        self.root
            .join(machine_id.to_string())
            .join("projects")
            .join(project_dir)
            .join(format!("{session_id}.jsonl"))
    }

    pub async fn write<R: AsyncRead + Unpin>(
        &self,
        machine_id: Uuid,
        project_dir: &str,
        session_id: &str,
        mut body: R,
    ) -> Result<Stats, ArchiveError> {
        validate_name(project_dir)?;
        validate_name(session_id)?;

        let final_path = self.path_of(machine_id, project_dir, session_id);
        let partial_path = final_path.with_extension("jsonl.partial");
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let outcome = async {
            let mut file = fs::File::create(&partial_path).await?;
            let mut hasher = Sha256::new();
            let mut size: u64 = 0;
            let mut line_count: u32 = 0;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = body.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n];
                hasher.update(chunk);
                size += n as u64;
                line_count += chunk.iter().filter(|&&b| b == b'\n').count() as u32;
                file.write_all(chunk).await?;
            }
            file.flush().await?;
            drop(file);
            Ok::<_, std::io::Error>(Stats {
                sha256: hex::encode(hasher.finalize()),
                size_bytes: size,
                line_count,
            })
        }
        .await;

        match outcome {
            Ok(stats) => {
                fs::rename(&partial_path, &final_path).await?;
                Ok(stats)
            }
            Err(e) => {
                let _ = fs::remove_file(&partial_path).await;
                Err(e.into())
            }
        }
    }
}

fn validate_name(s: &str) -> Result<(), ArchiveError> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains('/')
        || s.contains('\\')
        || s.contains('\0')
        || s.split('/').any(|seg| seg == "..")
    {
        return Err(ArchiveError::InvalidName);
    }
    Ok(())
}
```

Add to `crates/cctui-server/Cargo.toml` `[dependencies]`:
```
thiserror = "2"
```

Fix the placeholder `expected` sha in the test: replace the string with `hex::encode(sha2::Sha256::digest(body))` computed inline — change the test to:

```rust
        let expected = hex::encode(Sha256::digest(body));
        assert_eq!(stats.sha256, expected);
```

(and add `use sha2::{Digest, Sha256};` to the test module).

- [ ] **Step 4: Wire module**

Edit `crates/cctui-server/src/main.rs` at top:

```rust
mod archive_store;
```

Edit `crates/cctui-server/src/state.rs` (if not already, add field):

```rust
use std::sync::Arc;
use crate::archive_store::ArchiveStore;
// ...
pub archive: Arc<ArchiveStore>,
```

Edit `main.rs` `async fn main` after `let pool = ...`:

```rust
let archive_root = std::env::var("CCTUI_ARCHIVE_PATH")
    .unwrap_or_else(|_| "/archive".into());
let archive = std::sync::Arc::new(archive_store::ArchiveStore::new(archive_root.into()));
archive.ensure_root().await?;
```

Pass `archive: archive.clone(),` into `AppState { ... }`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p cctui-server archive_store`
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cctui-server/src/archive_store.rs crates/cctui-server/src/state.rs \
        crates/cctui-server/src/main.rs crates/cctui-server/Cargo.toml
git commit -m "feat(server): ArchiveStore with atomic writes + path validation (CCT-45)"
```

---

## Task 3: Archive routes — `HEAD` + `PUT` (TDD via integration tests)

**Files:**
- Create: `crates/cctui-server/src/routes/archive.rs`
- Modify: `crates/cctui-server/src/routes/mod.rs` (`pub mod archive;`)
- Modify: `crates/cctui-server/src/main.rs` (register routes + per-route body limit)
- Modify: `crates/cctui-server/tests/integration.rs` (flow tests)

- [ ] **Step 1: Write failing integration tests**

Append to `crates/cctui-server/tests/integration.rs` (follow pattern of existing tests — spawn server, create a user via admin, enroll to get a machine token). Add:

```rust
#[tokio::test]
async fn archive_head_404_then_put_200_then_head_204() {
    let app = spawn_test_server().await;
    let machine_token = app.enrol_test_machine().await;
    let body = b"{\"one\":1}\n{\"two\":2}\n";
    let sha = sha256_hex_bytes(body);

    // HEAD → 404 (absent)
    let head = app.client()
        .head(format!("{}/api/v1/archive/-home-user-foo/abc-123?sha256={sha}", app.base))
        .bearer_auth(&machine_token)
        .send().await.unwrap();
    assert_eq!(head.status(), 404);

    // PUT → 200
    let put = app.client()
        .put(format!("{}/api/v1/archive/-home-user-foo/abc-123", app.base))
        .bearer_auth(&machine_token)
        .header("X-CCTUI-SHA256", &sha)
        .body(body.to_vec())
        .send().await.unwrap();
    assert_eq!(put.status(), 200);
    let j: serde_json::Value = put.json().await.unwrap();
    assert_eq!(j["sha256"], sha);
    assert_eq!(j["size_bytes"], body.len());
    assert_eq!(j["line_count"], 2);

    // HEAD → 204
    let head2 = app.client()
        .head(format!("{}/api/v1/archive/-home-user-foo/abc-123?sha256={sha}", app.base))
        .bearer_auth(&machine_token)
        .send().await.unwrap();
    assert_eq!(head2.status(), 204);
}

#[tokio::test]
async fn archive_put_rejects_hash_mismatch() {
    let app = spawn_test_server().await;
    let machine_token = app.enrol_test_machine().await;
    let put = app.client()
        .put(format!("{}/api/v1/archive/-home-user-foo/abc-xyz", app.base))
        .bearer_auth(&machine_token)
        .header("X-CCTUI-SHA256", "00".repeat(32))
        .body(b"hello".to_vec())
        .send().await.unwrap();
    assert_eq!(put.status(), 409);
}

#[tokio::test]
async fn archive_put_rejects_path_traversal() {
    let app = spawn_test_server().await;
    let machine_token = app.enrol_test_machine().await;
    for evil in ["..", "../foo", "a/b"] {
        let url = format!("{}/api/v1/archive/{evil}/session-1",
            app.base, evil = urlencoding::encode(evil));
        let res = app.client().put(url).bearer_auth(&machine_token)
            .body(b"x".to_vec()).send().await.unwrap();
        assert_eq!(res.status(), 400, "expected 400 for project_dir {evil}");
    }
}

#[tokio::test]
async fn archive_requires_machine_role() {
    let app = spawn_test_server().await;
    let admin_token = app.admin_token();
    let res = app.client()
        .put(format!("{}/api/v1/archive/-home-foo/abc", app.base))
        .bearer_auth(&admin_token)
        .body(b"x".to_vec()).send().await.unwrap();
    assert_eq!(res.status(), 403);
}

fn sha256_hex_bytes(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}
```

If `spawn_test_server` and `enrol_test_machine` helpers don't already exist in the integration file, extend the existing test scaffolding (see `integration.rs` for the pattern used by CCT-36 tests — create a user via `/admin/users`, then call `/enroll` with that user token to mint a machine token; expose `admin_token()`/`enrol_test_machine()`/`client()`/`base` on a `TestApp` struct).

Add `urlencoding = "2"` to `[dev-dependencies]` if not present.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cctui-server --test integration archive_`
Expected: compilation failure (missing route) or 404 from server.

- [ ] **Step 3: Implement routes**

Create `crates/cctui-server/src/routes/archive.rs`:

```rust
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::io::StreamReader;
use uuid::Uuid;

use crate::archive_store::ArchiveError;
use crate::auth::{AuthContext, TokenRole};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct HeadQuery {
    pub sha256: String,
}

#[derive(Serialize)]
pub struct PutResponse {
    pub sha256: String,
    pub size_bytes: u64,
    pub line_count: u32,
}

fn require_machine(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    match (ctx.role, ctx.machine_id) {
        (TokenRole::Machine, Some(mid)) => Ok(mid),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

pub async fn head(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((project_dir, session_id)): Path<(String, String)>,
    Query(q): Query<HeadQuery>,
) -> Result<StatusCode, StatusCode> {
    let machine_id = require_machine(&ctx)?;
    if !valid_name(&project_dir) || !valid_name(&session_id) || q.sha256.len() != 64 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT sha256 FROM archive_index WHERE machine_id = $1 AND session_id = $2",
    )
    .bind(machine_id)
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match row {
        Some((hash,)) if hash.eq_ignore_ascii_case(&q.sha256) => Ok(StatusCode::NO_CONTENT),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn put(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((project_dir, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<PutResponse>, StatusCode> {
    let machine_id = require_machine(&ctx)?;
    if !valid_name(&project_dir) || !valid_name(&session_id) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let claimed_hash = headers
        .get("X-CCTUI-SHA256")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());

    let stream = body.into_data_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e)));
    let reader = StreamReader::new(stream);

    let stats = state.archive
        .write(machine_id, &project_dir, &session_id, reader)
        .await
        .map_err(|e| match e {
            ArchiveError::InvalidName => StatusCode::BAD_REQUEST,
            ArchiveError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    if let Some(claimed) = claimed_hash {
        if claimed != stats.sha256 {
            let _ = tokio::fs::remove_file(
                state.archive.path_of(machine_id, &project_dir, &session_id),
            )
            .await;
            return Err(StatusCode::CONFLICT);
        }
    }

    sqlx::query(
        "INSERT INTO archive_index \
         (machine_id, project_dir, session_id, sha256, size_bytes, line_count) \
         VALUES ($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (machine_id, session_id) DO UPDATE SET \
             sha256 = EXCLUDED.sha256, size_bytes = EXCLUDED.size_bytes, \
             line_count = EXCLUDED.line_count, uploaded_at = now(), \
             project_dir = EXCLUDED.project_dir",
    )
    .bind(machine_id)
    .bind(&project_dir)
    .bind(&session_id)
    .bind(&stats.sha256)
    .bind(stats.size_bytes as i64)
    .bind(stats.line_count as i32)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(PutResponse {
        sha256: stats.sha256,
        size_bytes: stats.size_bytes,
        line_count: stats.line_count,
    }))
}
```

Add to `cctui-server/Cargo.toml` `[dependencies]`:

```
futures-util = { workspace = true }
tokio-util = { version = "0.7", features = ["io"] }
```

Edit `crates/cctui-server/src/routes/mod.rs`:

```rust
pub mod archive;
```

Edit `crates/cctui-server/src/main.rs` — add routes to `api_router` (so they inherit `auth_middleware`):

```rust
.route(
    "/archive/{project_dir}/{session_id}",
    axum::routing::put(routes::archive::put)
        .head(routes::archive::head)
        .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
)
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cctui-server --test integration archive_`
Expected: 4 tests pass.

- [ ] **Step 5: Clippy + fmt**

Run: `make fmt && make lint`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/cctui-server/Cargo.toml \
        crates/cctui-server/src/routes/archive.rs \
        crates/cctui-server/src/routes/mod.rs \
        crates/cctui-server/src/main.rs \
        crates/cctui-server/tests/integration.rs
git commit -m "feat(server): HEAD/PUT /api/v1/archive routes with hash gate (CCT-45)"
```

---

## Task 4: Channel — `archive.ts` module (walker + hasher + orchestrator)

**Files:**
- Create: `channel/src/archive.ts`
- Create: `channel/test/archive.test.ts`

- [ ] **Step 1: Write failing walker/hasher unit tests**

Create `channel/test/archive.test.ts`:

```ts
import { describe, it, expect } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { walkProjectDirs, computeFileSha256 } from "../src/archive";

describe("archive", () => {
  it("walks projects dir and returns ProjectFile entries", () => {
    const root = mkdtempSync(join(tmpdir(), "cctui-"));
    const p1 = join(root, "-home-user-foo");
    mkdirSync(p1, { recursive: true });
    writeFileSync(join(p1, "abc-123.jsonl"), '{"x":1}\n');
    writeFileSync(join(p1, "README.txt"), "ignore");
    const p2 = join(root, "-home-user-bar");
    mkdirSync(p2);
    writeFileSync(join(p2, "def-456.jsonl"), '{"y":2}\n');

    const files = walkProjectDirs(root);
    const rels = files.map(f => `${f.projectDir}/${f.sessionId}`).sort();
    expect(rels).toEqual(["-home-user-bar/def-456", "-home-user-foo/abc-123"]);
  });

  it("computes stable sha256", async () => {
    const root = mkdtempSync(join(tmpdir(), "cctui-"));
    const path = join(root, "f.jsonl");
    writeFileSync(path, "hello\n");
    // sha256("hello\n") = 5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03
    expect(await computeFileSha256(path)).toBe(
      "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
    );
  });

  it("skips missing projects root without throwing", () => {
    expect(walkProjectDirs("/definitely/does/not/exist")).toEqual([]);
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd channel && bun test archive`
Expected: module not found.

- [ ] **Step 3: Implement `channel/src/archive.ts`**

```ts
import { createHash } from "node:crypto";
import { createReadStream, readdirSync, statSync, existsSync } from "node:fs";
import { join, basename } from "node:path";
import type { ServerBridge } from "./bridge";

export interface ProjectFile {
  absPath: string;
  projectDir: string;   // e.g. "-home-dorsk-Documents-foo"
  sessionId: string;    // filename without .jsonl
}

export type UploadOutcome = "skipped" | "uploaded" | "failed";

/** Walks <root>/<projectDir>/*.jsonl. Returns all files; never throws. */
export function walkProjectDirs(root: string): ProjectFile[] {
  if (!existsSync(root)) return [];
  const out: ProjectFile[] = [];
  let projects: string[] = [];
  try {
    projects = readdirSync(root);
  } catch {
    return [];
  }
  for (const projectDir of projects) {
    const projPath = join(root, projectDir);
    let st;
    try { st = statSync(projPath); } catch { continue; }
    if (!st.isDirectory()) continue;
    let entries: string[] = [];
    try { entries = readdirSync(projPath); } catch { continue; }
    for (const name of entries) {
      if (!name.endsWith(".jsonl")) continue;
      const absPath = join(projPath, name);
      try {
        if (!statSync(absPath).isFile()) continue;
      } catch { continue; }
      const sessionId = basename(name, ".jsonl");
      out.push({ absPath, projectDir, sessionId });
    }
  }
  return out;
}

export function computeFileSha256(absPath: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const h = createHash("sha256");
    const s = createReadStream(absPath);
    s.on("error", reject);
    s.on("data", (c) => h.update(c));
    s.on("end", () => resolve(h.digest("hex")));
  });
}

/** In-process cache: absPath → last uploaded sha256. */
const uploadedHash = new Map<string, string>();

export async function uploadIfChanged(
  bridge: ServerBridge,
  file: ProjectFile,
): Promise<UploadOutcome> {
  let sha: string;
  try {
    sha = await computeFileSha256(file.absPath);
  } catch (err) {
    console.error(`[cctui-channel] hash failed ${file.absPath}:`, err);
    return "failed";
  }
  if (uploadedHash.get(file.absPath) === sha) return "skipped";

  let state: "present" | "absent";
  try {
    state = await bridge.headArchive(file.projectDir, file.sessionId, sha);
  } catch (err) {
    console.error(`[cctui-channel] HEAD archive failed:`, err);
    return "failed";
  }
  if (state === "present") {
    uploadedHash.set(file.absPath, sha);
    return "skipped";
  }

  try {
    await bridge.putArchive(file.projectDir, file.sessionId, file.absPath, sha);
    uploadedHash.set(file.absPath, sha);
    return "uploaded";
  } catch (err) {
    console.error(`[cctui-channel] PUT archive failed for ${file.sessionId}:`, err);
    return "failed";
  }
}

export function __resetArchiveCacheForTests(): void {
  uploadedHash.clear();
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd channel && bun test archive`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add channel/src/archive.ts channel/test/archive.test.ts
git commit -m "feat(channel): archive walker + hasher + uploadIfChanged (CCT-45)"
```

---

## Task 5: Channel — bridge methods `headArchive` / `putArchive`

**Files:**
- Modify: `channel/src/bridge.ts`

- [ ] **Step 1: Add methods**

Append to `ServerBridge` class:

```ts
  async headArchive(
    projectDir: string,
    sessionId: string,
    sha256: string,
  ): Promise<"present" | "absent"> {
    const url = `${this.baseUrl}/api/v1/archive/${encodeURIComponent(projectDir)}/${encodeURIComponent(sessionId)}?sha256=${sha256}`;
    const res = await fetch(url, {
      method: "HEAD",
      headers: { Authorization: `Bearer ${this.token}` },
    });
    if (res.status === 204) return "present";
    if (res.status === 404) return "absent";
    throw new Error(`HEAD archive: unexpected ${res.status}`);
  }

  async putArchive(
    projectDir: string,
    sessionId: string,
    absPath: string,
    sha256: string,
  ): Promise<void> {
    const url = `${this.baseUrl}/api/v1/archive/${encodeURIComponent(projectDir)}/${encodeURIComponent(sessionId)}`;
    const file = Bun.file(absPath);
    const res = await fetch(url, {
      method: "PUT",
      headers: {
        Authorization: `Bearer ${this.token}`,
        "X-CCTUI-SHA256": sha256,
        "Content-Type": "application/octet-stream",
      },
      body: file.stream(),
    });
    if (!res.ok) {
      throw new Error(`PUT archive failed: ${res.status} ${await res.text()}`);
    }
  }
```

- [ ] **Step 2: Typecheck**

Run: `cd channel && bun run tsc --noEmit` (or project's typecheck script)
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add channel/src/bridge.ts
git commit -m "feat(channel): headArchive/putArchive bridge methods (CCT-45)"
```

---

## Task 6: Channel — wire startup scan, periodic, final flush

**Files:**
- Modify: `channel/src/index.ts`

- [ ] **Step 1: Add imports + scheduler after session match**

At top of `channel/src/index.ts` add:

```ts
import { homedir } from "node:os";
import { join as pathJoin } from "node:path";
import { walkProjectDirs, uploadIfChanged } from "./archive";
```

After the block that starts transcript tailing (inside `registerAndWaitForSession` once `matched = true` and session is set), add:

```ts
        // --- Archive pipeline ---
        const projectsRoot = process.env.CLAUDE_PROJECTS_DIR
          ?? pathJoin(homedir(), ".claude", "projects");

        const currentAbs = session.transcriptPath ?? null;
        const archiveOnce = async (opts?: { skipCurrent?: boolean }) => {
          const files = walkProjectDirs(projectsRoot);
          for (const f of files) {
            if (opts?.skipCurrent && currentAbs && f.absPath === currentAbs) continue;
            await uploadIfChanged(bridge, f);
          }
        };

        // Startup scan — fire-and-forget; skip current session (periodic covers it).
        archiveOnce({ skipCurrent: true }).catch((err) =>
          console.error("[cctui-channel] startup archive scan failed:", err),
        );

        // Periodic flush for live session.
        const intervalMin = Number(process.env.CCTUI_ARCHIVE_INTERVAL_MINUTES ?? 15);
        const intervalMs = Math.max(1, intervalMin) * 60_000;
        const periodic = setInterval(async () => {
          if (!currentAbs) return;
          await uploadIfChanged(bridge, {
            absPath: currentAbs,
            projectDir: require("node:path").basename(require("node:path").dirname(currentAbs)),
            sessionId: require("node:path").basename(currentAbs, ".jsonl"),
          });
        }, intervalMs);

        // Expose for SIGTERM/SIGINT handlers below.
        (globalThis as any).__cctuiPeriodic = periodic;
        (globalThis as any).__cctuiCurrentAbs = currentAbs;
```

- [ ] **Step 2: Update signal handlers**

Replace the existing `process.on("SIGTERM", ...)` and `process.on("SIGINT", ...)` blocks with:

```ts
async function finalFlush(): Promise<void> {
  tailAbort?.abort();
  bridge.stopPolling();
  const periodic = (globalThis as any).__cctuiPeriodic as ReturnType<typeof setInterval> | undefined;
  if (periodic) clearInterval(periodic);
  const currentAbs = (globalThis as any).__cctuiCurrentAbs as string | undefined;
  if (currentAbs) {
    const { basename, dirname } = await import("node:path");
    await uploadIfChanged(bridge, {
      absPath: currentAbs,
      projectDir: basename(dirname(currentAbs)),
      sessionId: basename(currentAbs, ".jsonl"),
    }).catch((err) => console.error("[cctui-channel] final archive flush failed:", err));
  }
}

process.on("SIGTERM", async () => { await finalFlush(); process.exit(0); });
process.on("SIGINT",  async () => { await finalFlush(); process.exit(0); });
```

- [ ] **Step 3: Typecheck + manual smoke (server running)**

```bash
cd channel && bun run tsc --noEmit
```
Expected: clean.

Manual: build bundle (`bun run build`), start server, run a Claude session against a small fake `~/.claude/projects/` tree, watch server logs for `PUT /api/v1/archive/...` 200s and verify files land under `$CCTUI_ARCHIVE_PATH/<machine_uuid>/projects/...`.

- [ ] **Step 4: Commit**

```bash
git add channel/src/index.ts
git commit -m "feat(channel): startup/periodic/final JSONL archive upload (CCT-45)"
```

---

## Task 7: Config doc + STATUS update

**Files:**
- Modify: `STATUS.md`
- Modify: `Makefile` (add `CCTUI_ARCHIVE_PATH` default for `run/server`)

- [ ] **Step 1: Makefile default**

Find the `run/server` target and add `CCTUI_ARCHIVE_PATH=$(PWD)/.archive` to its env (alongside existing env vars). Add `.archive/` to `.gitignore`.

- [ ] **Step 2: STATUS.md**

Add a "JSONL archive" bullet under the appropriate "Done" section summarising: per-machine upload to PVC, hash-gated, 15-min periodic + on-exit flush, restore via rsync.

- [ ] **Step 3: Commit**

```bash
git add STATUS.md Makefile .gitignore
git commit -m "docs(CCT-45): status + archive path default"
```

---

## Task 8: Full verification

- [ ] **Step 1: Run all tests**

```bash
make test/unit
cargo test -p cctui-server
cd channel && bun test
```
Expected: green.

- [ ] **Step 2: fmt + lint**

```bash
make fmt && make lint
```
Expected: clean.

- [ ] **Step 3: Push branch + open PR**

```bash
git push -u origin CCT-45-jsonl-archive
gh pr create --title "feat: JSONL per-machine archive sync to PVC (CCT-45)" \
  --body "$(cat <<'EOF'
## Summary
- New `archive_index` table + `HEAD`/`PUT /api/v1/archive/{project_dir}/{session_id}` routes (Machine-role only).
- `ArchiveStore` streams body to `.partial`, sha256 + size + line_count in one pass, atomic rename, path-traversal rejected.
- Channel walks `~/.claude/projects/**/*.jsonl` at startup, re-uploads current session every 15 min, and flushes on SIGTERM/SIGINT. Hash-gated via HEAD 204/404.
- Restore = `rsync <pvc>/<machine_uuid>/projects/ ~/.claude/projects/`.

## Test plan
- [ ] `cargo test -p cctui-server` — unit + integration (archive flow, 409 on hash mismatch, 400 on path traversal, 403 for non-Machine).
- [ ] `cd channel && bun test` — walker + hasher.
- [ ] Manual: two-machine smoke — verify files appear at `<CCTUI_ARCHIVE_PATH>/<machine_uuid>/projects/...` and `archive_index` has rows.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-review notes

- **Spec coverage**: migration (T1), archive_store (T2), routes HEAD/PUT with body limit + hash header + traversal rejection (T3), channel walker/hasher/uploadIfChanged (T4), bridge methods (T5), startup/periodic/final triggers (T6), config + STATUS (T7). GET `/archive/index` marked optional in the spec — not implemented here; add in a follow-up ticket only if needed.
- **Machine auth source**: server-side, `machine_id` derives from `AuthContext` (CCT-36 PR1). Channel-side uses whatever token `loadConfig()` returns — works with CCTUI_AGENT_TOKEN today, transparently upgrades to machine key once CCT-36 PR2 merges (no change to this plan).
- **SIGKILL**: unhandled by design — startup scan on next run catches the completed file.
- **Body-size cap**: 100 MiB applied as `DefaultBodyLimit::max` layer on the archive route only; oversize → 413 from axum.
- **Placeholders**: none — every step has real code or a concrete command.
