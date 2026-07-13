import type { Account } from "./account.ts";

export const REACTION_CONTENTS = [
  "+1",
  "-1",
  "laugh",
  "confused",
  "heart",
  "hooray",
  "rocket",
  "eyes",
] as const;

export type ReactionContent = (typeof REACTION_CONTENTS)[number];

export interface ReactionSummary {
  "+1": number;
  "-1": number;
  laugh: number;
  hooray: number;
  confused: number;
  heart: number;
  rocket: number;
  eyes: number;
  total_count: number;
  viewer_reactions: ReactionContent[];
}

interface RawReaction {
  id: number;
  content: string;
  user?: { login?: string } | null;
}

function isReactionContent(value: string): value is ReactionContent {
  return (REACTION_CONTENTS as readonly string[]).includes(value);
}

async function listReactions(octokit: Account["octokit"], base: string): Promise<RawReaction[]> {
  const all: RawReaction[] = [];
  for (let page = 1; page <= 20; page++) {
    const res = await octokit.request(`GET ${base}/reactions`, { per_page: 100, page });
    const batch = Array.isArray(res.data) ? (res.data as RawReaction[]) : [];
    all.push(...batch);
    if (batch.length < 100) break;
  }
  return all;
}

function summarize(reactions: RawReaction[], login: string): ReactionSummary {
  const counts: Record<ReactionContent, number> = {
    "+1": 0,
    "-1": 0,
    laugh: 0,
    confused: 0,
    heart: 0,
    hooray: 0,
    rocket: 0,
    eyes: 0,
  };
  const viewer: ReactionContent[] = [];
  let total = 0;
  for (const r of reactions) {
    if (!isReactionContent(r.content)) continue;
    counts[r.content] += 1;
    total += 1;
    if (r.user?.login === login && !viewer.includes(r.content)) viewer.push(r.content);
  }
  return { ...counts, total_count: total, viewer_reactions: viewer };
}

export async function toggleReaction(
  octokit: Account["octokit"],
  base: string,
  login: string,
  content: ReactionContent,
): Promise<ReactionSummary> {
  const existing = await listReactions(octokit, base);
  const own = existing.find((r) => r.user?.login === login && r.content === content);
  if (own) {
    await octokit.request(`DELETE ${base}/reactions/${own.id}`);
  } else {
    await octokit.request(`POST ${base}/reactions`, { content });
  }
  const after = await listReactions(octokit, base);
  return summarize(after, login);
}
