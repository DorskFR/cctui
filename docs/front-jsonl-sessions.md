# Front JSONL Sessions

Design for tracking hand-run Claude JSONL conversations alongside background
sessions.

## Goals

- Treat the JSONL transcript as the durable conversation identity.
- Display recent foreground conversations without creating noise from old files.
- Promote a foreground conversation to a managed background session when the
  user sends a message from cctui.

## Discovery

The daemon should own discovery because it already knows the Claude projects
root, transcript encoding, offsets, and parser. A foreground scan should:

- enumerate project directories under the configured Claude projects root;
- read only `*.jsonl` transcript files modified within a bounded recency window;
- ignore transcripts already represented by a live/background session id;
- parse the first useful transcript rows lazily for cwd, title, timestamps, and
  preview text;
- register foreground rows with metadata `{ "origin": "front" }`.

The scan should be incremental: persist a small cursor containing path, mtime,
size, and transcript id so repeated polls avoid reparsing unchanged files.

## Model

Use the existing `sessions` table and metadata JSON rather than adding a second
inventory. Foreground rows need:

- `id`: transcript session id;
- `working_dir`: decoded project cwd;
- `status`: `inactive`;
- `adapter_id`: `claude-code`;
- `metadata.origin`: `front`;
- `metadata.transcript_path_hint`: optional basename/project-relative hint only,
  not an absolute local path.

The UI can show these as top-level conversations with a `front` badge. They are
not subagents and should not be nested by `parent_id`.

## Promotion

On first UI message to a foreground row, the server should dispatch a resume
command to the owning daemon:

1. Resolve the foreground row to its machine and cwd.
2. Ask the daemon to create a background worker with `--resume <session_id>`.
3. Keep the same session id and transcript.
4. After the worker reports alive, deliver the queued message through the normal
   reply path.

If the user also has the transcript open in a foreground TUI, promotion should
not mutate or delete the foreground process. It only creates a managed background
attacher for future cctui messages.

## Follow-Up Implementation Slices

1. Daemon foreground scanner with recency limit and dedupe.
2. Server registration/listing fields and `origin=front` badge in the UI.
3. Promote-on-message command that resumes and then replies.
4. Archive/filter controls for foreground rows once volume is known.
