import type { Account } from "./account.ts";

export type ReviewState = "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "PENDING";

export interface RawReview {
  user: string | null;
  avatar_url: string | null;
  state: string;
}

export interface PullRef {
  owner: string;
  repo: string;
  number: number;
}

const REVIEWS_PER_PAGE = 100;
const REVIEWS_MAX_PAGES = 20;

const VERDICTS = new Set(["APPROVED", "CHANGES_REQUESTED", "DISMISSED"]);

export function reduceReviewStates(
  reviews: RawReview[],
): Map<string, { avatar_url: string | null; state: ReviewState }> {
  const out = new Map<string, { avatar_url: string | null; state: ReviewState }>();
  for (const r of reviews) {
    if (!r.user) continue;
    const state = r.state.toUpperCase();
    const prev = out.get(r.user);
    const avatar_url = r.avatar_url ?? prev?.avatar_url ?? null;
    if (VERDICTS.has(state)) {
      out.set(r.user, { avatar_url, state: state as ReviewState });
    } else if (state === "COMMENTED") {
      if (!prev || prev.state === "COMMENTED") {
        out.set(r.user, { avatar_url, state: "COMMENTED" });
      } else {
        out.set(r.user, { avatar_url, state: prev.state });
      }
    }
  }
  return out;
}

export async function fetchPullReviews(
  octokit: Account["octokit"],
  p: PullRef,
): Promise<RawReview[]> {
  const reviews: RawReview[] = [];
  for (let page = 1; page <= REVIEWS_MAX_PAGES; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/reviews", {
      owner: p.owner,
      repo: p.repo,
      pull_number: p.number,
      per_page: REVIEWS_PER_PAGE,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as Record<string, unknown>[]) : [];
    for (const rv of batch) {
      const user = (rv.user as { login?: string; avatar_url?: string } | undefined) ?? undefined;
      reviews.push({
        user: user?.login ?? null,
        avatar_url: user?.avatar_url ?? null,
        state: String(rv.state ?? ""),
      });
    }
    if (batch.length < REVIEWS_PER_PAGE) break;
  }
  return reviews;
}
