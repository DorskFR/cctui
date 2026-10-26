# Images in stream events

Screenshots and other base64 attachments (a `Read` of a PNG, an MCP screenshot,
a pasted document) are stored once in `daemon_blobs`, keyed by the sha256 of
their bytes, and shared by every session that shows them. The event payload
keeps a reference in place of the base64 `source`:

```json
{"type": "image", "source": {"type": "cctui-blob", "blob_id": "<sha256>", "size": 412345, "media_type": "image/png"}}
```

This is the marker the daemon already writes for sources over 512 KiB; the
server now applies it to every base64 source of 1 KiB or more. Because
`search_text` is generated from the payload, it no longer carries the base64
either.

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
| `CCTUI_IMAGE_BACKFILL_PAUSE_MS` | `1000` | sleep between batches |
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
```

Restart from the beginning (idempotent: rewritten rows are skipped):

```sql
UPDATE image_blob_backfill SET last_event_id = 0 WHERE name = 'backfill';
```

## Durable file backend

Set `CCTUI_BLOB_PATH` to a directory on durable storage shared by every server
replica. Blob metadata, ownership links and hashes remain in PostgreSQL; new
blob bytes are atomically written and synced to `<directory>/<sha256>`. The
same authenticated routes and image references serve both backends. Reads
verify the file length and hash. Missing or damaged files fail closed.

Without the variable, new blobs continue using PostgreSQL. Existing database
blobs remain readable with either configuration. Once any `file_stored` rows
exist, keep the directory mounted and the variable configured, including on
restore. Back up this directory together with PostgreSQL. Files whose database
transaction rolled back are safe orphans; this change does not delete files.
Do not remove the directory or roll back migration 191 while it is in use.

To migrate existing blobs, copy and verify each file by hash and length before
setting `file_stored = true, bytes = ''::bytea` in a transaction. Preserve the
original rows in a verified external backup first. Keep plugin archives and
image recovery on the same backend-aware reader.
