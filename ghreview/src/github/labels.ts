import type { Account } from "./account.ts";

export interface Label {
  name: string;
  color: string;
  description: string | null;
}

interface RawLabel {
  name?: string;
  color?: string;
  description?: string | null;
}

function normalize(raw: RawLabel[]): Label[] {
  return raw
    .filter((l): l is RawLabel & { name: string } => typeof l.name === "string")
    .map((l) => ({
      name: l.name,
      color: typeof l.color === "string" ? l.color : "",
      description: l.description ?? null,
    }));
}

export async function listRepoLabels(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
): Promise<Label[]> {
  const all: RawLabel[] = [];
  for (let page = 1; page <= 20; page++) {
    const res = await octokit.request(`GET /repos/${owner}/${repo}/labels`, {
      per_page: 100,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as RawLabel[]) : [];
    all.push(...batch);
    if (batch.length < 100) break;
  }
  return normalize(all);
}

export async function addPullLabel(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
  name: string,
): Promise<Label[]> {
  const res = await octokit.request(`POST /repos/${owner}/${repo}/issues/${number}/labels`, {
    labels: [name],
  });
  const data = Array.isArray(res.data) ? (res.data as RawLabel[]) : [];
  return normalize(data);
}

export async function removePullLabel(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
  name: string,
): Promise<Label[]> {
  const res = await octokit.request(
    `DELETE /repos/${owner}/${repo}/issues/${number}/labels/${encodeURIComponent(name)}`,
  );
  const data = Array.isArray(res.data) ? (res.data as RawLabel[]) : [];
  return normalize(data);
}
