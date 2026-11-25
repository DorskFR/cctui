# Images in stream events

Screenshots and other base64 attachments (a `Read` of a PNG, an MCP screenshot,
a pasted document) are stored once in `daemon_blobs`, keyed by the sha256 of
their bytes, and shared by every session that shows them. The event payload
keeps a reference in place of the base64 `source`:

```json
{"type": "image", "source": {"type": "cctui-blob", "blob_id": "<sha256>", "size": 412345, "media_type": "image/png"}}
```

This is the marker the daemon already writes for sources over 512 KiB; the
server now applies it to every base64 source of 1 KiB or more.

Only valid images are stored as images. A source declaring `image/png`,
`image/jpeg`, `image/gif` or `image/webp` must decode from base64, start with
the magic bytes of one of those formats, and fully decode; the reference
carries the format the bytes actually are. Other media types only need to be
base64.

Secret redaction no longer scans encoded attachment data (the `data` of a
base64 source, `data:…;base64,` URLs, blob references). Rows redacted before
that carry `[REDACTED:square_token]` inside their base64 and no longer decode.

A source that fails is kept losslessly but never served as an image: the blob
holds the base64 text verbatim and the reference says why.

```json
{"type": "cctui-blob", "blob_id": "<sha256 of the text>", "size": 403112, "media_type": "image/png",
 "encoding": "base64", "corrupt": true, "corrupt_reason": "redacted"}
```

| `corrupt_reason` | Meaning |
| --- | --- |
| `redacted` | a redaction placeholder inside the base64 |
| `not_base64` | does not decode from base64 |
| `bad_magic` | declares png/jpeg/gif/webp, bytes are none of them |
| `undecodable` | the image is truncated or damaged |

### Recovery

A `redacted` source is recovered when exactly one image blob reachable from a
session of the same user is its intact original: same size range, the text
before the first placeholder and after the last one match, every kept run in
between appears in order, and the blob decodes. The reference then points at
that blob and carries `"recovered": true`; the redacted text is not stored.
Two matches, or none, leave it `corrupt`. Recovery runs at ingest and in the
backfill.

A session may read a blob only through a `session_blob_links` row, which is
written for every reference an inserted event carries:
`GET /api/v1/sessions/{id}/blobs/{hash}`.

## Ingest

Daemon `Message`/`ToolUse` events are rewritten before they are stored and
broadcast. Each blob is written and then checked (`sha256(bytes)` must equal its
key) before the payload points at it. If any write or check fails, the event
is stored inline as before. A replay of an event the session already holds
inline (stored before the backfill reached it) is left inline too, so the dedup
indexes still suppress it.

## Read paths

| Consumer | What it gets |
| --- | --- |
| webui conversation | text blocks as the result body, images below it from the blob route (old inline rows render from their base64); `[image image/png 403 KB, unreadable]` for corrupt data |
| webui export (HTML / Markdown) | text blocks, `[image image/png 403 KB]` per image |
| `CctuiHistory` / cctuiverse transcript | the same labels |
| search chunks | text blocks only (unchanged) |
| `GET /api/v1/sessions/{id}/conversation` | references, by default |
| `GET …/conversation?inline_images=true` | base64 `source`s, up to 8 MiB of decoded image bytes per page; images past that, and corrupt ones, stay references |
| TUI | the result text as before (now the short reference instead of base64) |

## Backfill

Rows stored before this change are rewritten by a background walk. It is
**off by default**.

Each event is handled in one transaction: validate and recover its sources,
write the blobs, verify each by hash, grant the session its links, then replace
the payload only if `content_hash` still matches the row that was read. The walk
also revisits references stored opaque before validation existed (an
`"encoding": "base64"` ref without `"corrupt": true`): each is recovered or
flagged. Any error rolls the whole event back and
the batch stops before the cursor moves, so the next batch retries it. Blobs are
never deleted. An event left inline is logged (`image backfill left an event
inline`) and skipped:

- a stored blob fails its hash check;
- the row changed in the meantime;
- the rewritten payload equals another row of the session (a replayed
  duplicate).

The cursor and counters live in `image_blob_backfill` (`name = 'backfill'`):
`images_valid`, `images_corrupt` and `images_recovered` count sources by
verdict, and each batch logs them.
One replica walks at a time (advisory lock). The walk stays 30 s behind the
newest row, then polls every minute.

| Variable | Default | Effect |
| --- | --- | --- |
| `CCTUI_IMAGE_BACKFILL` | off | `1`/`true`/`on`/`yes` starts the walk |
| `CCTUI_IMAGE_BACKFILL_SCAN` | `500` | events scanned per batch |
| `CCTUI_IMAGE_BACKFILL_ROWS` | `20` | events rewritten per batch (about 10 MB of screenshots) |
| `CCTUI_IMAGE_BACKFILL_CONCURRENCY` | `1` | events of a batch rewritten at once, each on its own pooled connection; keep it well under `CCTUI_DB_MAX_CONNECTIONS` (4 with the default pool of 20) |
| `CCTUI_IMAGE_BACKFILL_PAUSE_MS` | `1000` | sleep between batches; `0` for none |
| `CCTUI_IMAGE_BACKFILL_BUDGET_MB` | `512` | new blob bytes this process writes before the walk stops; restart to continue |

Disk grows by the new blob bytes until autovacuum frees the old TOAST chunks of
the rewritten rows: a few hundred MB at a time under the default budget. Take a
snapshot first, and run `VACUUM (VERBOSE) stream_events` between budgets if
autovacuum lags.

Progress:

```sql
SELECT last_event_id, rows_rewritten, pg_size_pretty(blob_bytes) AS blob_bytes,
       images_valid, images_corrupt, images_recovered, updated_at
FROM image_blob_backfill WHERE name = 'backfill';

SELECT count(*), pg_size_pretty(sum(byte_len)) FROM daemon_blobs;

-- inline image payloads left
-- (pg_column_size reads no TOAST, so only large rows are detoasted)
SELECT count(*), pg_size_pretty(sum(pg_column_size(payload)))
FROM stream_events
WHERE pg_column_size(payload) > 65536 AND strpos(payload::text, '"type": "base64"') > 0;
```

Restart from the beginning (idempotent: rewritten rows are skipped):

```sql
UPDATE image_blob_backfill SET last_event_id = 0 WHERE name = 'backfill';
```

## External blob stores

By default blob bytes live in `daemon_blobs.bytes`. One external store can
hold them instead; metadata, ownership links and hashes stay in PostgreSQL. A
row with `file_stored = true` has empty `bytes` and its bytes in the external
store under its sha256 (`daemon_blobs.hash`). Every reader (the session blob
route, plugin archives, image recovery and ingest verification) goes through
the same backend-aware reader, which checks the length and the sha256 and
fails closed on a missing or damaged object. Existing database rows stay
readable with any configuration.

The store is chosen once at boot. Setting both `CCTUI_BLOB_PATH` and
`CCTUI_BLOB_S3_BUCKET`, or the bucket without its endpoint and keys, refuses
to start.

| Variable | Default | Effect |
| --- | --- | --- |
| `CCTUI_BLOB_PATH` | unset | file backend: a directory on durable storage shared by every replica; bytes are written atomically and synced to `<directory>/<sha256>` |
| `CCTUI_BLOB_S3_BUCKET` | unset | S3 backend: the bucket |
| `CCTUI_BLOB_S3_ENDPOINT` | required with the bucket | `http(s)://host[:port]`, e.g. `http://versitygw.storage.svc:7070`; requests are path-style, `<endpoint>/<bucket>/<key>` |
| `CCTUI_BLOB_S3_REGION` | `us-east-1` | SigV4 signing region |
| `CCTUI_BLOB_S3_ACCESS_KEY_ID` | required with the bucket | static access key |
| `CCTUI_BLOB_S3_SECRET_ACCESS_KEY` | required with the bucket | static secret key |
| `CCTUI_BLOB_S3_PREFIX` | empty | object key is `<prefix><sha256>` |

A write checks that the bytes hash to their key before storing them. On S3 an
object already present with the same length is not uploaded again. When a row
with external bytes is deleted (a plugin uninstall), its object is deleted
after the commit, unless the hash was stored again meanwhile; a failed delete
is only logged. Objects whose database transaction rolled back are safe
orphans.

Once any `file_stored` rows exist, keep the store configured, including on
restore, and back it up together with PostgreSQL. Do not roll back migration
191 while it is in use.

### Moving existing blobs

`CCTUI_BLOB_MOVE=1` starts a background task that moves the bytes of every
database-stored blob to the configured external store. It is **off by
default**, and logs and exits when no external store is configured. One
replica runs it (advisory lock); the others exit quietly.

For each blob, in hash order: check that the bytes hash to the key and match
`byte_len` (a mismatch is logged once and skipped for the life of the process,
the row left as is), write it to the store, read it back and verify it, then
`UPDATE daemon_blobs SET file_stored = true, bytes = ''::bytea WHERE hash = $1
AND NOT file_stored` in its own transaction. A restart resumes where it
stopped: moved rows no longer match. A progress line is logged every 100 blobs
(`blob move progress`: moved, bytes, skipped, remaining) and a summary at the
end (`blob move finished`).

| Variable | Default | Effect |
| --- | --- | --- |
| `CCTUI_BLOB_MOVE` | off | `1`/`true`/`on`/`yes` starts the move |
| `CCTUI_BLOB_MOVE_BATCH` | `20` | rows read per batch |
| `CCTUI_BLOB_MOVE_PAUSE_MS` | `500` | sleep between batches |
| `CCTUI_BLOB_MOVE_BUDGET_MB` | unlimited | stop after this many MB moved by this process; restart to continue |

Procedure:

1. Take a PostgreSQL snapshot.
2. Configure the S3 variables and deploy. New blobs now go to S3.
3. Set `CCTUI_BLOB_MOVE=1` and deploy. Watch the logs and the SQL below until
   nothing is left but skipped rows.
4. Remove `CCTUI_BLOB_MOVE` and deploy.
5. `VACUUM (FULL, VERBOSE) daemon_blobs;` to return the space to the volume.
   It locks the table while it rewrites it; blob reads and writes wait.

Progress:

```sql
SELECT file_stored, count(*), pg_size_pretty(sum(byte_len)) AS bytes,
       pg_size_pretty(sum(octet_length(bytes))) AS in_postgres
FROM daemon_blobs GROUP BY file_stored;

SELECT pg_size_pretty(pg_total_relation_size('daemon_blobs'));
```

Rollback: rows already moved, and blobs written since step 2, are readable
only while S3 stays configured. Stopping the move (step 4) is always safe.
Unsetting the S3 variables is safe only while no row has `file_stored = true`;
otherwise each object must first be copied back into `bytes`, verified by hash,
and `file_stored` cleared in the same transaction.
