# Batched daemon-ingest bench

Reproducible harness for the before/after numbers on `stream_events` insertion.

Daemon ingest used to unpack each leaf of a frame and await a separate
`INSERT INTO stream_events` per event. It now groups the persistable events of
one frame into a single multi-row `INSERT ... SELECT FROM unnest(...) WITH
ORDINALITY ... ON CONFLICT DO NOTHING`, one statement per 1000 rows, keeping a
per-row `seq` so publish order still follows the frame.

## Dataset

Built by the bench itself, against a migrated database — no fixture to load. It
seeds one machine and two owned sessions, then backfills **5000 events** into
each: one session through the per-row path, one through the batched path. Two
sessions rather than one, so the dedup indexes never suppress the second arm's
rows and both arms do identical work.

The events are the same shape the daemon emits for a transcript replay: an
`assistant` message payload, distinct per event, no turn id.

## Run

The harness is the timing test next to the code it measures, so it exercises the
real path — the ownership guard, the NUL strip, the link extraction and the
`INSERT_BATCH` chunking, not a hand-written approximation of the SQL:

```sh
createdb cctui_bench
DATABASE_URL=postgres://…/cctui_bench sqlx migrate run --source migrations
DATABASE_URL=postgres://…/cctui_bench cargo test -p cctui-server --lib \
  routes::daemon::ingest -- --ignored --nocapture batched_backfill
```

It is `#[ignore]`d: it is slow, and its numbers only mean something on an
otherwise quiet machine. It fails under a 10x speedup, which is the acceptance
criterion the batching was written against.

## Results

> **TODO — not yet measured.** Run the command above and paste the printed
> timings here, then mirror the summary into `docs/benchmarks.md`.

| path | statements | wall time | events/s |
|---|---:|---:|---:|
| One `INSERT` per event | 5000 | TODO | TODO |
| Batched | 5 | TODO | TODO |
| Speedup | | TODO | |

## What the numbers should show

The per-row arm pays one network round-trip per event, each maintaining the GIN
trigram index and the dedup unique index, and each holding that daemon's WS
reader for its duration. The batched arm pays five. The expected win is
therefore round-trip latency times 4995, not index work — index maintenance is
the same volume either way, which is why the speedup is a function of how far
the database is from the server rather than of the dataset size.

A run against a local socket is the *pessimistic* case for the batching: the
further the database, the larger the gap.
