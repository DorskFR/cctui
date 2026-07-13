import type { DbHandle } from "./client.ts";

export type ReviewVerdict = "comment" | "approve" | "request_changes";
export type ReviewSide = "LEFT" | "RIGHT";

export interface ReviewDraftComment {
  id: string;
  path: string;
  side: ReviewSide;
  line: number;
  start_line: number | null;
  start_side: ReviewSide | null;
  body: string;
  created_at: string | null;
  updated_at: string | null;
}

export interface ReviewDraft {
  id: string;
  account: string;
  owner: string;
  repo: string;
  pr_number: number;
  head_sha: string | null;
  verdict: ReviewVerdict;
  body: string;
  created_at: string | null;
  updated_at: string | null;
  comments: ReviewDraftComment[];
}

export interface PullRef {
  owner: string;
  repo: string;
  number: number;
}

const DRAFT_COLUMNS = `
  id::text, account, owner, repo, pr_number, head_sha, verdict, body,
  to_char(created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at,
  to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
`;

const COMMENT_COLUMNS = `
  id::text, path, side, line, start_line, start_side, body,
  to_char(created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at,
  to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
`;

interface DraftRow {
  id: string;
  account: string;
  owner: string;
  repo: string;
  pr_number: number;
  head_sha: string | null;
  verdict: ReviewVerdict;
  body: string;
  created_at: string | null;
  updated_at: string | null;
}

async function loadComments(db: DbHandle, draftId: string): Promise<ReviewDraftComment[]> {
  const { sql } = db;
  return sql<ReviewDraftComment[]>`
    SELECT ${sql.unsafe(COMMENT_COLUMNS)}
    FROM review_draft_comments
    WHERE draft_id = ${draftId}
    ORDER BY id
  `;
}

async function hydrate(db: DbHandle, row: DraftRow): Promise<ReviewDraft> {
  return { ...row, comments: await loadComments(db, row.id) };
}

/** Fetch the caller-owned draft for a PR, or null if none exists / not owned. */
export async function getDraft(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
): Promise<ReviewDraft | null> {
  const { sql } = db;
  const [row] = await sql<DraftRow[]>`
    SELECT ${sql.unsafe(DRAFT_COLUMNS)}
    FROM review_drafts d
    WHERE d.account = ${account}
      AND d.owner = ${ref.owner}
      AND d.repo = ${ref.repo}
      AND d.pr_number = ${ref.number}
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.id = d.account_id AND ga.user_id = ${userId}
      )
    LIMIT 1
  `;
  return row ? hydrate(db, row) : null;
}

/**
 * Get or create the caller-owned draft for a PR. Returns null when the caller
 * does not own the account. head_sha is captured only on first creation.
 */
export async function openDraft(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
  headSha: string | null,
): Promise<ReviewDraft | null> {
  const { sql } = db;
  const [row] = await sql<DraftRow[]>`
    WITH owned AS (
      SELECT id FROM gh_accounts WHERE login = ${account} AND user_id = ${userId}
    )
    INSERT INTO review_drafts (account_id, account, owner, repo, pr_number, head_sha)
    SELECT owned.id, ${account}, ${ref.owner}, ${ref.repo}, ${ref.number}, ${headSha}
    FROM owned
    ON CONFLICT (account_id, owner, repo, pr_number) DO UPDATE
      SET head_sha = COALESCE(review_drafts.head_sha, EXCLUDED.head_sha),
          updated_at = now()
    RETURNING ${sql.unsafe(DRAFT_COLUMNS)}
  `;
  return row ? hydrate(db, row) : null;
}

export interface DraftMeta {
  verdict?: ReviewVerdict;
  body?: string;
}

/** Update draft-level verdict/body. Returns the refreshed draft or null. */
export async function updateDraftMeta(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
  meta: DraftMeta,
): Promise<ReviewDraft | null> {
  const { sql } = db;
  const [row] = await sql<DraftRow[]>`
    UPDATE review_drafts d
    SET verdict = COALESCE(${meta.verdict ?? null}, d.verdict),
        body = COALESCE(${meta.body ?? null}, d.body),
        updated_at = now()
    WHERE d.account = ${account}
      AND d.owner = ${ref.owner} AND d.repo = ${ref.repo} AND d.pr_number = ${ref.number}
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.id = d.account_id AND ga.user_id = ${userId}
      )
    RETURNING ${sql.unsafe(DRAFT_COLUMNS)}
  `;
  return row ? hydrate(db, row) : null;
}

export interface NewComment {
  path: string;
  side: ReviewSide;
  line: number;
  start_line?: number | null;
  start_side?: ReviewSide | null;
  body: string;
}

/** Insert a comment into the caller-owned draft. Returns the refreshed draft. */
export async function addDraftComment(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
  headSha: string | null,
  input: NewComment,
): Promise<ReviewDraft | null> {
  const draft = await openDraft(db, userId, account, ref, headSha);
  if (!draft) return null;
  const { sql } = db;
  await sql`
    INSERT INTO review_draft_comments (draft_id, path, side, line, start_line, start_side, body)
    VALUES (${draft.id}, ${input.path}, ${input.side}, ${input.line},
            ${input.start_line ?? null}, ${input.start_side ?? null}, ${input.body})
  `;
  await sql`UPDATE review_drafts SET updated_at = now() WHERE id = ${draft.id}`;
  return getDraft(db, userId, account, ref);
}

export interface EditComment {
  body?: string;
  line?: number;
  side?: ReviewSide;
  start_line?: number | null;
  start_side?: ReviewSide | null;
}

/** Edit a comment, scoped to the caller-owned draft for the PR. */
export async function editDraftComment(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
  commentId: string,
  patch: EditComment,
): Promise<ReviewDraft | null> {
  const draft = await getDraft(db, userId, account, ref);
  if (!draft) return null;
  const { sql } = db;
  const rows = await sql<{ id: string }[]>`
    UPDATE review_draft_comments c
    SET body = COALESCE(${patch.body ?? null}, c.body),
        line = COALESCE(${patch.line ?? null}, c.line),
        side = COALESCE(${patch.side ?? null}, c.side),
        start_line = ${patch.start_line === undefined ? sql`c.start_line` : patch.start_line},
        start_side = ${patch.start_side === undefined ? sql`c.start_side` : patch.start_side},
        updated_at = now()
    WHERE c.id = ${commentId} AND c.draft_id = ${draft.id}
    RETURNING c.id::text
  `;
  if (rows.length === 0) return null;
  return getDraft(db, userId, account, ref);
}

/** Delete a comment from the caller-owned draft. Returns the refreshed draft. */
export async function deleteDraftComment(
  db: DbHandle,
  userId: string,
  account: string,
  ref: PullRef,
  commentId: string,
): Promise<ReviewDraft | null> {
  const draft = await getDraft(db, userId, account, ref);
  if (!draft) return null;
  const { sql } = db;
  const rows = await sql<{ id: string }[]>`
    DELETE FROM review_draft_comments
    WHERE id = ${commentId} AND draft_id = ${draft.id}
    RETURNING id::text
  `;
  if (rows.length === 0) return null;
  return getDraft(db, userId, account, ref);
}

/** Remove the whole draft (and its comments) after a successful publish. */
export async function clearDraft(db: DbHandle, draftId: string): Promise<void> {
  await db.sql`DELETE FROM review_drafts WHERE id = ${draftId}`;
}

export async function deleteReviewDraftsForPull(
  db: DbHandle,
  account: string,
  ref: PullRef,
): Promise<void> {
  await db.sql`
    DELETE FROM review_drafts
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pr_number = ${ref.number}
  `;
}
