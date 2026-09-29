import type { GithubPull } from "../api/types";

export interface ReviewPromptInput {
  owner: string;
  repo: string;
  pull: GithubPull;
  skill?: string | null;
}

export function pullUrl(owner: string, repo: string, pull: GithubPull): string {
  return pull.html_url ?? `https://github.com/${owner}/${repo}/pull/${pull.number}`;
}

export function headSha(pull: GithubPull): string | null {
  return pull.cctui_enriched_head_sha ?? pull.head?.sha ?? null;
}

// The head SHA is named so the session reviews the revision that was on screen,
// not wherever the branch has moved since.
export function buildReviewPrompt({
  owner,
  repo,
  pull,
  skill = "gh-review",
}: ReviewPromptInput): string {
  const sha = headSha(pull);
  const lines = [
    `Review pull request ${owner}/${repo}#${pull.number}: ${pull.title}`,
    "",
    `- URL: ${pullUrl(owner, repo, pull)}`,
    `- Head: ${pull.head?.ref ?? "unknown"}${sha ? ` at ${sha}` : ""}`,
    `- Base: ${pull.base?.ref ?? "unknown"}`,
  ];
  if (pull.changed_files != null) {
    lines.push(`- Changed files: ${pull.changed_files}`);
  }
  lines.push("");
  lines.push(
    sha
      ? `Review the diff at head SHA ${sha} and report concrete, actionable defects with file and line references.`
      : "Review the diff at the head of the pull request and report concrete, actionable defects with file and line references.",
  );
  if (skill) {
    lines.push(
      `If the ${skill} skill is available, use it to draft review comments so the drafts land back in the review centre.`,
    );
  }
  return lines.join("\n");
}
